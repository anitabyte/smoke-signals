use image::ImageReader;
use lru::LruCache;
use smoke_signals::data_structures::*;
use smoke_signals::{BASE_FS_CACHE_PATH, CACHE_ENTRY_SEPARATOR, LB_TOKEN, get_cache_key};
use std::{
    env,
    fs::read_dir,
    io::{self, Cursor},
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use thiserror::Error;
use tokio::time::MissedTickBehavior;
use tokio::{
    fs::create_dir_all,
    io::AsyncWriteExt,
    join,
    net::{TcpListener, TcpStream},
    sync::watch::{self, Receiver, Sender},
    task::spawn_blocking,
};
use tracing::{error, info, warn};

#[derive(Error, Debug)]
enum FetchError {
    #[error("no now playing available")]
    NotPlaying,
    #[error("Album art not available for release {0}")]
    AlbumArtNotAvailable(String),
    #[error("http error")]
    ReqwestError(#[from] reqwest::Error),
    #[error("uri parse error")]
    UriParseError(#[from] url::ParseError),
    #[error("io error")]
    IoError(#[from] io::Error),
    #[error("image error")]
    ImageError(#[from] image::ImageError),
    #[error("tokio join error")]
    JoinError(#[from] tokio::task::JoinError),
    #[error("hex decode error")]
    Utf8Error(#[from] std::string::FromUtf8Error),
    #[error("hex decode error")]
    HexError(#[from] hex::FromHexError),
}

#[derive(Error, Debug)]
enum ServerError {
    #[error("Receive error")]
    ReceiveError(#[from] watch::error::RecvError),
    #[error("IO error")]
    IoError(#[from] std::io::Error),
    #[error("Serialisation error")]
    SerialisationError(#[from] serde_json::Error),
}

#[tracing::instrument(skip(client))]
async fn get_now_listening(
    client: &reqwest::Client,
    auth_token: &str,
) -> Result<TrackMetadata, FetchError> {
    let response = client
        .get("https://api.listenbrainz.org/1/user/finallyworn/playing-now")
        .header("Authorization", auth_token)
        .send()
        .await?;
    let structured_json = response.json::<NowListening>().await?;
    match structured_json.payload.listens.first() {
        None => Err(FetchError::NotPlaying),
        Some(first) => Ok(first.track_metadata.clone()),
    }
}

#[tracing::instrument(skip(client))]
async fn get_mbid_id(
    client: &reqwest::Client,
    track_metadata: &TrackMetadata,
    auth_token: &str,
) -> Result<String, FetchError> {
    let uri = "https://api.listenbrainz.org/1/metadata/lookup/";
    let params = [
        ("artist_name", &track_metadata.artist_name),
        ("recording_name", &track_metadata.track_name),
        ("release_name", &track_metadata.release_name),
    ];
    let uri_with_params = reqwest::Url::parse_with_params(uri, params)?;
    let response = client
        .get(uri_with_params)
        .header("Authorization", auth_token)
        .send()
        .await?;
    let structured_json_response = response.json::<Lookup>().await?;
    Ok(structured_json_response.release_mbid)
}

#[tracing::instrument(skip(client))]
async fn get_album_art_metadata(
    client: &reqwest::Client,
    mbid: &str,
) -> Result<CoverArtMetadata, FetchError> {
    let uri = format!("https://coverartarchive.org/release/{}", mbid);
    let response = client.get(uri).send().await?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(FetchError::AlbumArtNotAvailable(mbid.to_string()));
    }
    let response = response.error_for_status()?;
    Ok(response.json::<CoverArtMetadata>().await?)
}

#[tracing::instrument(skip(client))]
async fn get_album_art_data(client: &reqwest::Client, uri: &str) -> Result<Vec<u8>, FetchError> {
    let response = client.get(uri).send().await?;
    let image_bytes = response.bytes().await?.into();
    Ok(image_bytes)
}

#[tracing::instrument(skip(bytes))]
async fn resize_image(bytes: Arc<Vec<u8>>) -> Result<Vec<u8>, FetchError> {
    tokio::task::spawn_blocking(move || {
        let img = ImageReader::new(Cursor::new(bytes.as_ref()))
            .with_guessed_format()?
            .decode()?;
        let resized_image = img.resize_to_fill(64, 64, image::imageops::FilterType::Gaussian);
        let rgb_values = resized_image.to_rgb8().into_raw_bgr().clone();
        Ok(rgb_values)
    })
    .await?
}

fn get_file_cache_paths(path: impl AsRef<Path>) -> Vec<PathBuf> {
    let Ok(entries) = read_dir(path) else {
        return vec![];
    };
    entries
        .flatten()
        .flat_map(|entry| {
            let Ok(meta) = entry.metadata() else {
                return vec![];
            };
            if meta.is_file() {
                return vec![entry.path()];
            }
            vec![]
        })
        .collect()
}

async fn get_file_content_from_fs(path: &PathBuf) -> Result<Vec<u8>, FetchError> {
    let file_content = tokio::fs::read(path).await?;
    Ok(file_content)
}

async fn get_cache_key_from_path(path: &Path) -> Result<(String, String, String), FetchError> {
    let cache_key_hex = path
        .file_name()
        .unwrap_or_default()
        .to_str()
        .unwrap_or_default();
    let cache_key = String::from_utf8(hex::decode(cache_key_hex)?)?;
    let split_string: Vec<&str> = cache_key.split(CACHE_ENTRY_SEPARATOR).collect();
    warn!("{:?}", cache_key);
    warn!("{:?}", cache_key_hex);
    let artist = split_string.get(0).unwrap_or(&"").to_string();
    let release = split_string.get(1).unwrap_or(&"").to_string();
    Ok((cache_key_hex.to_string(), artist, release))
}

async fn populate_cache_from_fs(
    cache: &mut LruCache<String, AlbumData>,
) -> Result<bool, FetchError> {
    let paths_handle = spawn_blocking(|| get_file_cache_paths(BASE_FS_CACHE_PATH));
    let paths = paths_handle.await?;
    for path in paths {
        let cache_key = get_cache_key_from_path(&path).await?;
        info!("Cache key: {}", cache_key.0);
        info!("Artist: {}", cache_key.1);
        info!("Release: {}", cache_key.2);
        let file_content = get_file_content_from_fs(&path).await?;
        let file_content_resized = resize_image(Arc::new(file_content)).await?;
        let ad = AlbumData {
            artist: cache_key.1,
            release: cache_key.2,
            art: file_content_resized.into(),
        };
        cache.put(cache_key.0, ad);
    }
    Ok(true)
}

async fn retrieve_from_cache(
    track_metadata: &TrackMetadata,
    inmem_cache: &mut LruCache<String, AlbumData>,
    cache_key: &str,
) -> Option<AlbumData> {
    // first we check the LruCache
    if let Some(ad) = inmem_cache.get(cache_key) {
        info!(
            "Album art for {} - {} retrieved from Lru",
            ad.artist, ad.release
        );
        Some(ad.clone())
    } else {
        let cache_file_path: PathBuf = [BASE_FS_CACHE_PATH, cache_key].into_iter().collect();
        if let Ok(file_content) = get_file_content_from_fs(&cache_file_path).await {
            // we've retrieved the original artwork: let's do our image transform
            let art = resize_image(file_content.into()).await.ok()?;
            let fs_ad = AlbumData::new(
                track_metadata.get_artist_name().to_string(),
                track_metadata.get_release_name().to_string(),
                art,
            );
            inmem_cache.put(cache_key.to_string(), fs_ad.clone());
            info!(
                "Album art for {} - {} retrieved from disk, inserted into Lru",
                fs_ad.artist, fs_ad.release
            );
            Some(fs_ad)
        } else {
            None
        }
    }
}

async fn write_to_fs_cache(
    metadata_provider: &impl MetadataProvider,
    original_image_data: Arc<Vec<u8>>,
) -> Result<(), FetchError> {
    let mut path = PathBuf::new();
    path.push(BASE_FS_CACHE_PATH);
    path.push(get_cache_key(metadata_provider).await);
    tokio::fs::write(path, original_image_data.as_ref()).await?;
    Ok(())
}

#[tracing::instrument(skip(client, cache))]
async fn get_album_art_pipeline(
    client: &reqwest::Client,
    cache: &mut LruCache<String, AlbumData>,
    auth_token: &str,
) -> Result<AlbumData, FetchError> {
    let now_listening = get_now_listening(client, auth_token).await?;
    let cache_key: String = get_cache_key(&now_listening).await;
    match retrieve_from_cache(&now_listening, cache, &cache_key).await {
        Some(i) => {
            info!("Found cached entry: {}", i.release);
            Ok(i.clone())
        }
        None => {
            info!("No cached entry found");
            let mbid = get_mbid_id(client, &now_listening, auth_token).await?;
            let album_art_metadata = get_album_art_metadata(client, &mbid).await?;
            let album_art_uri = &album_art_metadata
                .images
                .first()
                .ok_or(FetchError::AlbumArtNotAvailable(mbid))?
                .image;
            let album_art_data = get_album_art_data(client, album_art_uri).await?;
            let album_art_arc = Arc::new(album_art_data);
            let sixtyfour_data = resize_image(album_art_arc.clone()).await?;
            let ad = AlbumData::new(
                now_listening.get_artist_name().to_string(),
                now_listening.get_release_name().to_string(),
                sixtyfour_data,
            );
            write_to_fs_cache(&now_listening, album_art_arc.clone()).await?;
            cache.put(cache_key, ad.clone());
            Ok(ad)
        }
    }
}

#[tracing::instrument(skip(tx))]
async fn start_album_art_loop(tx: Sender<AlbumData>, auth_token: String) {
    let client = reqwest::Client::new();
    let mut cache: LruCache<String, AlbumData> = LruCache::new(NonZeroUsize::new(100).unwrap());

    let mut interval = tokio::time::interval(Duration::from_secs(1));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut current_album_data_key: Option<String> = None;
    loop {
        interval.tick().await;
        let album_data = get_album_art_pipeline(&client, &mut cache, &auth_token).await;
        match album_data {
            Ok(ad) => {
                let new_data_key: String = get_cache_key(&ad).await;
                match &current_album_data_key {
                    Some(cad) if cad.eq(&new_data_key) => {
                        warn!("Not updating current_album_data")
                    }
                    _ => {
                        current_album_data_key = Some(new_data_key);
                        tx.send(ad).ok();
                    }
                }
            }
            Err(e) => match e {
                FetchError::NotPlaying => warn!("Nothing playing"),
                FetchError::ReqwestError(e) => {
                    warn!("Error communicating with ListenBrainz: {}", e)
                }
                FetchError::UriParseError(e) => warn!("Error parsing Uri: {}", e),
                FetchError::AlbumArtNotAvailable(e) => {
                    warn!("No album art available for release {}", e)
                }
                _ => warn!("{:?}", e),
            },
        }
    }
}

#[tracing::instrument]
async fn server(rx: Receiver<AlbumData>) {
    let listener = TcpListener::bind("0.0.0.0:8080")
        .await
        .expect("Couldn't bind to port");

    loop {
        if let Ok((socket, _)) = listener.accept().await {
            let cloned_rx = rx.clone();
            tokio::spawn(handle_conn(socket, cloned_rx));
        } else {
            warn!("Failed to accept TcpStream");
        }
    }
}

#[tracing::instrument(skip(rx))]
async fn handle_conn(mut socket: TcpStream, mut rx: Receiver<AlbumData>) {
    info!("Connection accepted!");
    loop {
        let connection_result = connection_loop(&mut socket, &mut rx).await;
        match connection_result {
            Ok(()) => {}
            Err(ServerError::IoError(e)) => {
                warn!(
                    "Some IoError occurred: {:?}. Dropping connection to be safe",
                    e
                );
                break;
            }
            Err(e) => {
                error!(
                    "Some non-IO connection loop issue happened: {:?}. Dropping connection to be safe",
                    e
                );
                break;
            }
        }
    }
}

#[tracing::instrument(skip(rx))]
async fn connection_loop(
    socket: &mut TcpStream,
    rx: &mut Receiver<AlbumData>,
) -> Result<(), ServerError> {
    let album_data = rx.borrow_and_update().clone();
    let message_data = MessageData::from(album_data);
    socket.write_all(Vec::from(message_data).as_slice()).await?;
    rx.changed().await?;
    Ok(())
}

#[tokio::main]
pub async fn main() {
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber).unwrap();
    let cache_path = Path::new(BASE_FS_CACHE_PATH);

    let auth_token =
        env::var(LB_TOKEN).expect("Please specify the AUTH_TOKEN environment variable");

    let auth_token = format!("Token {}", auth_token);
    if !cache_path.is_dir() && cache_path.exists() {
        panic!("Your base cache path isn't a directory but does exist")
    }
    create_dir_all(cache_path)
        .await
        .expect("Unable to create base cache path, ending execution");
    info!("Created base cache path at {}", cache_path.display());
    let (tx, rx) = watch::channel(AlbumData {
        artist: String::new(),
        release: String::new(),
        art: Arc::new([]),
    });
    println!("Hello, world!");
    let server = tokio::spawn(server(rx));
    let album_art = tokio::spawn(start_album_art_loop(tx, auth_token.clone()));

    join!(server, album_art);
}
