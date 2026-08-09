pub mod data_structures;

use crate::data_structures::*;

pub const LB_TOKEN: &str = "LB_TOKEN";
pub const BASE_FS_CACHE_PATH: &str = "cache";
pub const CACHE_ENTRY_SEPARATOR: &str = "|-|-|";

pub async fn get_cache_key(metadata: &impl MetadataProvider) -> String {
    // We only really care about 'releases' from the point of view of album art - no need for get_recording_name
    hex::encode(format!(
        "{}{}{}",
        metadata.get_artist_name(),
        CACHE_ENTRY_SEPARATOR,
        metadata.get_release_name(),
    ))
}
