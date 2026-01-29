/// MIME detection via buffer sniffing with header fallback.
pub fn detect_mime(_buffer: &[u8], _headers: Option<&str>) -> String {
    todo!("sniff magic bytes, fall back to content-type header")
}

pub fn extension_for_mime(mime: &str) -> &str {
    match mime {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "audio/ogg" => "ogg",
        "audio/mpeg" => "mp3",
        "video/mp4" => "mp4",
        _ => "bin",
    }
}
