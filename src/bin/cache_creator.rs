use id3::frame::Picture;
use id3::{Tag, TagLike};
use smoke_signals::data_structures::AlbumData;
use smoke_signals::get_cache_key;
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

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

fn read_tag_image(path: impl AsRef<Path> + std::fmt::Debug) -> Result<AlbumData, id3::Error> {
    let tag = Tag::read_from_path(&path)?;
    let pictures = tag.pictures().collect::<Vec<&Picture>>();
    if let Some(picture) = pictures.first() {
        return Ok(AlbumData {
            artist: tag.artist().unwrap_or_default().into(),
            release: tag.album().unwrap_or_default().into(),
            art: picture.data.clone().into(),
        });
    }
    println!("{:?} fell through the artist bit", path);
    Ok(AlbumData {
        artist: String::new(),
        release: String::new(),
        art: vec![].into(),
    })
}

fn get_mp3_file_list(base_path: PathBuf) -> io::Result<Vec<PathBuf>> {
    let mut mp3_list: Vec<PathBuf> = Vec::new();
    let mut directories_to_visit: Vec<PathBuf> = vec![base_path];
    while let Some(dir) = directories_to_visit.pop() {
        let entries = fs::read_dir(dir)?;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                directories_to_visit.push(path);
            } else {
                mp3_list.push(path)
            }
        }
    }
    Ok(mp3_list)
}

#[tokio::main]
pub async fn main() {
    let base_path: PathBuf = [BASE_MP3_PATH].iter().collect();
    let mp3_paths = get_mp3_file_list(base_path);
    match mp3_paths {
        Ok(mp3s) => {
            println!("{:?}", mp3s);
            let set_parents = mp3s
                .clone()
                .into_iter()
                .map(|x| x.parent().unwrap().to_path_buf())
                .collect::<HashSet<PathBuf>>();
            println!("{:?}", set_parents);
            let mut parents_with_images: HashSet<PathBuf> = HashSet::new();

            for mp3 in mp3s {
                if !parents_with_images.contains(mp3.parent().unwrap_or("".as_ref())) {
                    let ad = read_tag_image(&mp3).unwrap_or_default();
                    if !ad.art.is_empty() {
                        parents_with_images.insert(mp3.parent().unwrap().to_path_buf());
                        create_image_in_staging_cache(&ad).await;
                    }
                }
            }
        }
        Err(e) => {
            println!("Oh no! {:?}", e)
        }
    }
    println!("Hello")
}
