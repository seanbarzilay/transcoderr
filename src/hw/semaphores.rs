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
        Self { by_key: Arc::new(map) }
    }

    /// Build an empty registry (no devices).
    pub fn empty() -> Self {
        Self { by_key: Arc::new(HashMap::new()) }
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
