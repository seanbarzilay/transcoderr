use transcoderr::hw::probe::parse_encoders_listing;

#[test]
fn finds_known_hw_encoders_in_listing() {
    let stdout = r#"
 V..... h264               H.264 / AVC / MPEG-4 AVC / MPEG-4 part 10
 V..... h264_nvenc         NVIDIA NVENC H.264 encoder
 V..... hevc_qsv           HEVC (Intel Quick Sync Video acceleration)
 V..... libx264            H.264 (libx264)
"#;
    let found = parse_encoders_listing(stdout);
    assert!(found.contains(&"h264_nvenc"));
    assert!(found.contains(&"hevc_qsv"));
    assert!(!found.contains(&"hevc_vaapi"));
}
