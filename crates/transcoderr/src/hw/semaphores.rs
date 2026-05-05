use super::devices::{Accel, HwCaps};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Semaphore;

#[derive(Clone)]
pub struct DeviceRegistry {
    by_key: Arc<HashMap<String, Arc<Semaphore>>>, // key: "nvenc:0"
}

impl DeviceRegistry {
    pub fn from_caps(caps: &HwCaps) -> Self {
        let mut map: HashMap<String, Arc<Semaphore>> = HashMap::new();
        for d in &caps.devices {
            map.insert(
                format!("{}:{}", d.accel.as_str(), d.index),
                Arc::new(Semaphore::new(d.max_concurrent as usize)),
            );
        }
        Self {
            by_key: Arc::new(map),
        }
    }

    /// Build an empty registry (no devices).
    pub fn empty() -> Self {
        Self {
            by_key: Arc::new(HashMap::new()),
        }
    }

    /// Acquire from the first available preferred accel.
    /// Returns the device key + permit, or None if nothing is free.
    pub async fn acquire_preferred(
        &self,
        prefer: &[Accel],
    ) -> Option<(String, tokio::sync::OwnedSemaphorePermit)> {
        for accel in prefer {
            let prefix = format!("{}:", accel.as_str());
            for (key, sem) in self.by_key.iter().filter(|(k, _)| k.starts_with(&prefix)) {
                if let Ok(permit) = sem.clone().try_acquire_owned() {
                    return Some((key.clone(), permit));
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hw::devices::{Accel, Device, HwCaps};

    fn caps_with_nvenc(max: u32) -> HwCaps {
        HwCaps {
            probed_at: 0,
            ffmpeg_version: None,
            devices: vec![Device {
                accel: Accel::Nvenc,
                index: 0,
                name: "test".into(),
                max_concurrent: max,
            }],
            encoders: vec!["hevc_nvenc".into()],
        }
    }

    /// Empty caps → empty registry → `acquire_preferred` returns None
    /// regardless of prefer list. This is the failure mode we hit in
    /// the worker daemon when `from_caps(&HwCaps::default())` was wired
    /// instead of the actual probe result.
    #[tokio::test]
    async fn empty_caps_yield_no_permits() {
        let reg = DeviceRegistry::from_caps(&HwCaps::default());
        assert!(reg.acquire_preferred(&[Accel::Nvenc]).await.is_none());
    }

    /// Caps with one NVENC device → registry hands out up to
    /// max_concurrent permits, then refuses further until one drops.
    #[tokio::test]
    async fn nvenc_caps_yield_max_concurrent_permits() {
        let reg = DeviceRegistry::from_caps(&caps_with_nvenc(3));
        let p1 = reg.acquire_preferred(&[Accel::Nvenc]).await;
        let p2 = reg.acquire_preferred(&[Accel::Nvenc]).await;
        let p3 = reg.acquire_preferred(&[Accel::Nvenc]).await;
        let p4 = reg.acquire_preferred(&[Accel::Nvenc]).await;
        assert!(p1.is_some());
        assert!(p2.is_some());
        assert!(p3.is_some());
        assert!(p4.is_none(), "4th acquire should fail with max=3");
        drop(p1);
        let p5 = reg.acquire_preferred(&[Accel::Nvenc]).await;
        assert!(p5.is_some(), "permit should be reusable after drop");
    }
}
