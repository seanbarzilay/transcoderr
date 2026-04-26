use transcoderr::hw::{
    devices::{Accel, Device, HwCaps},
    semaphores::DeviceRegistry,
};

#[tokio::test]
async fn semaphore_blocks_after_max_concurrent() {
    let caps = HwCaps {
        probed_at: 0,
        ffmpeg_version: None,
        devices: vec![Device {
            accel: Accel::Nvenc,
            index: 0,
            name: "n0".into(),
            max_concurrent: 1,
        }],
        encoders: vec![],
    };
    let reg = DeviceRegistry::from_caps(&caps);
    let (k1, p1) = reg
        .acquire_preferred(&[Accel::Nvenc])
        .await
        .expect("first acquires");
    assert_eq!(k1, "nvenc:0");
    let none = reg.acquire_preferred(&[Accel::Nvenc]).await;
    assert!(none.is_none(), "second should fail because limit=1");
    drop(p1);
    let (_, _) = reg
        .acquire_preferred(&[Accel::Nvenc])
        .await
        .expect("after drop, free again");
}
