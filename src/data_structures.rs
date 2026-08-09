use std::sync::Arc;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct NowListening {
    pub payload: Payload,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Payload {
    pub count: u8,
    pub user_id: String,
    pub listens: Vec<Listen>,
    pub playing_now: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Listen {
    pub track_metadata: TrackMetadata,
}

pub trait MetadataProvider {
    fn get_artist_name(&self) -> &str;
    fn get_release_name(&self) -> &str;
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TrackMetadata {
    pub artist_name: String,
    pub release_name: String,
    pub track_name: String,
}

impl MetadataProvider for TrackMetadata {
    fn get_artist_name(&self) -> &str {
        self.artist_name.as_str()
    }
    fn get_release_name(&self) -> &str {
        self.release_name.as_str()
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Lookup {
    pub artist_credit_name: String,
    pub artist_mbids: Vec<String>,
    pub recording_mbid: String,
    pub recording_name: String,
    pub release_mbid: String,
    pub release_name: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct CoverArtMetadata {
    pub images: Vec<CAImages>,
    pub release: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct CAImages {
    pub approved: bool,
    pub back: bool,
    pub comment: String,
    pub front: bool,
    pub image: String,
}

#[derive(Serialize, Debug, Clone, PartialEq, Default)]
pub struct AlbumData {
    pub artist: String,
    pub release: String,
    pub art: Arc<[u8]>,
}

impl MetadataProvider for AlbumData {
    fn get_artist_name(&self) -> &str {
        self.artist.as_str()
    }

    fn get_release_name(&self) -> &str {
        self.release.as_str()
    }
}

impl AlbumData {
    pub fn new(artist: String, release: String, art: Vec<u8>) -> AlbumData {
        AlbumData {
            artist,
            release,
            art: art.into(),
        }
    }
}

pub struct MessageData {
    pub total_len: u16,
    pub artist_len: u8,
    pub artist: String,
    pub release_len: u8,
    pub release: String,
    pub art_len: u16,
    pub art: Arc<[u8]>,
}

impl From<AlbumData> for MessageData {
    fn from(album_data: AlbumData) -> MessageData {
        let artist_len = album_data.artist.len() as u8;
        let artist: String = album_data.artist;
        let release_len = album_data.release.len() as u8;
        let release = album_data.release;
        let art_len: u16 = album_data.art.len() as u16;
        let total_len = (2 * size_of::<u16>() as u16)
            + (2 * size_of::<u8>() as u16)
            + art_len
            + artist_len as u16
            + release_len as u16;
        MessageData {
            total_len,
            artist_len,
            artist,
            release_len,
            release,
            art_len,
            art: album_data.art,
        }
    }
}

impl From<MessageData> for Vec<u8> {
    fn from(md: MessageData) -> Vec<u8> {
        let mut ret_vec: Vec<u8> = Vec::with_capacity(md.total_len as usize);
        let total_len = md.total_len.to_be_bytes();
        ret_vec.extend_from_slice(&total_len);
        ret_vec.push(md.artist_len);
        ret_vec.extend_from_slice(md.artist.as_bytes());
        ret_vec.push(md.release_len);
        ret_vec.extend_from_slice(md.release.as_bytes());
        ret_vec.extend_from_slice(&md.art_len.to_be_bytes());
        ret_vec.extend_from_slice(&md.art);
        ret_vec
    }
}
