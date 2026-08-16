use id3::frame::Picture;
use id3::{Tag, TagLike};
use smoke_signals::data_structures::AlbumData;
use smoke_signals::get_cache_key;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, Write};
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use tokio::task::JoinSet;

const BASE_MP3_PATH: &str = "D:\\MP3";
const TARGET_CACHE_PATH: &str = "staging_cache";

async fn create_image_in_staging_cache(ad: &AlbumData) -> io::Result<()> {
    let cache_key = get_cache_key(ad).await;
    let mut full_path = PathBuf::new();
    full_path.push(TARGET_CACHE_PATH);
    full_path.push(cache_key);
    let mut file = File::create(full_path)?;
    file.write_all(&ad.art)?;
    Ok(())
}

fn read_tag_image(
    path: impl AsRef<Path> + std::fmt::Debug,
) -> Result<Option<AlbumData>, id3::Error> {
    let tag = Tag::read_from_path(&path)?;
    let pictures = tag.pictures().collect::<Vec<&Picture>>();
    if let Some(picture) = pictures.first() {
        return Ok(Some(AlbumData {
            artist: tag.artist().unwrap_or_default().into(),
            release: tag.album().unwrap_or_default().into(),
            art: picture.data.clone().into(),
        }));
    }
    //println!("{:?} fell through the artist bit", path);
    Ok(None)
}

fn get_directory_leaf_list(base_path: PathBuf) -> io::Result<Vec<PathBuf>> {
    let mut dir_list: Vec<PathBuf> = Vec::new();
    let mut directories_to_visit: Vec<PathBuf> = vec![base_path];
    while let Some(dir) = directories_to_visit.pop() {
        let mut is_leaf_directory = true;
        let entries = fs::read_dir(&dir)?;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                is_leaf_directory = false;
                directories_to_visit.push(path);
            }
        }
        if is_leaf_directory {
            dir_list.push(dir)
        }
    }
    Ok(dir_list)
}

async fn leaf_directory_file_list(leaf_directory: PathBuf) -> io::Result<Vec<PathBuf>> {
    let mut entries = tokio::fs::read_dir(leaf_directory).await?;
    let mut file_list: Vec<PathBuf> = vec![];
    while let Some(entry) = entries.next_entry().await? {
        file_list.push(entry.path());
    }
    Ok(file_list)
}

async fn get_album_art(leaf_directory: PathBuf) -> Option<AlbumData> {
    let files_in_leaf = leaf_directory_file_list(leaf_directory).await.ok()?;
    // we prefer album art sourced from ID3 tags first, so we'll try to find the first MP3
    // in our file list and attempt to fetch album art from there
    let mp3 = files_in_leaf
        .iter()
        .filter(|x| x.extension() == Some(OsStr::new("mp3")))
        .take(1)
        .collect::<Vec<&PathBuf>>();
    if !mp3.is_empty() {
        let mp3_clone = mp3.first()?.to_owned().clone();
        // our read_tag stuff is sync, so let's spawn::blocking
        let blocking_tag_read = tokio::task::spawn_blocking(move || read_tag_image(mp3_clone))
            .await
            .ok()?
            .ok()?;
        if let Some(image) = blocking_tag_read {
            create_image_in_staging_cache(&image).await.ok()?;
            Some(image)
        } else {
            // now let's check for jpegs in the same directory in the same directory
            let jpgs = files_in_leaf
                .iter()
                .filter(|x| {
                    x.extension()
                        .unwrap_or_default()
                        .eq_ignore_ascii_case("jpg")
                })
                .map(|x| (x, std::fs::metadata(x).unwrap().file_size()))
                .max_by(|x, y| x.1.cmp(&y.1))?;
            let jpg_file_name = jpgs.0;
            let jpg_contents = tokio::fs::read(jpg_file_name).await.ok()?;
            let components = jpg_file_name
                .components()
                .map(|x| x.as_os_str().to_str().unwrap_or_default().to_string())
                .collect::<Vec<String>>();
            // we assume we have a file structure that is `artist/album/filename`
            let components_suffix = &components[components.len() - 3..components.len() - 1];
            let image = AlbumData {
                artist: components_suffix
                    .first()
                    .unwrap_or(&String::new())
                    .to_owned(),
                release: components_suffix
                    .get(1)
                    .unwrap_or(&String::new())
                    .to_owned(),
                art: jpg_contents.into(),
            };
            create_image_in_staging_cache(&image).await.ok()?;
            Some(image)
        }
    } else {
        None
    }
}

#[tokio::main]
pub async fn main() {
    let base_path: PathBuf = [BASE_MP3_PATH].iter().collect();
    let mp3_leaf_folders = get_directory_leaf_list(base_path);
    match mp3_leaf_folders {
        Ok(folders) => {
            let mut handles = JoinSet::new();
            for folder in folders {
                handles.spawn(async { get_album_art(folder).await });
            }
            handles.join_all().await;
        }
        Err(e) => {
            println!("We shouldn't be here: {:?}", e);
        }
    }
}
