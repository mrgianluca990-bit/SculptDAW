//! SCULPT_CORE22_SHARED_AUDIO: a simple atomic shared audio profile for Show and DAW.
//! Both apps read ~/Library/Application Support/Sculpt/AudioSettings.conf.
//! Hardware UID is authoritative for Show; DAW matches the physical output's
//! unique display name through CPAL 0.16 (which does not expose a UID API).
//! If names are duplicated, DAW refuses the switch instead of guessing.
use std::path::PathBuf;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AudioSettings {
    pub output_uid: String,
    pub output_name: String,
    pub input_name: String,
    pub sample_rate: u32,
    pub buffer_frames: u32,
}
impl AudioSettings {
    pub fn file() -> Option<PathBuf> {
        #[cfg(target_arch = "wasm32")]
        {
            None
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            std::env::var_os("HOME").map(PathBuf::from).map(|h| h.join("Library/Application Support/Sculpt/AudioSettings.conf"))
        }
    }
    pub fn read() -> Self {
        let Some(file) = Self::file() else { return Self::default() };
        let Ok(s) = std::fs::read_to_string(file) else { return Self::default() };
        let mut data = Self::default();
        if s.lines().next() != Some("SCULPT_AUDIO_V1") {
            return data;
        }
        for line in s.lines().skip(1) {
            let Some((key, value)) = line.split_once('=') else { continue };
            match key {
                "uid" => data.output_uid = value.to_string(),
                "name" => data.output_name = value.to_string(),
                "input" => data.input_name = value.to_string(),
                "rate" => data.sample_rate = value.parse().unwrap_or(0),
                "buffer" => data.buffer_frames = value.parse().unwrap_or(0),
                _ => (),
            }
        }
        data
    }
    pub fn save(&self) -> Result<(), String> {
        let path = Self::file().ok_or("HOME not set")?;
        let dir = path.parent().ok_or("missing config parent")?;
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let clean = |s: &str| s.replace(['\r', '\n'], " ");
        let body = format!(
            "SCULPT_AUDIO_V1\nuid={}\nname={}\ninput={}\nrate={}\nbuffer={}\n",
            clean(&self.output_uid),
            clean(&self.output_name),
            clean(&self.input_name),
            self.sample_rate,
            self.buffer_frames
        );
        let tmp = path.with_extension("conf.tmp");
        std::fs::write(&tmp, body).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }
}

#[derive(Clone, Debug)]
pub struct AudioDeviceInfo {
    pub name: String,
    pub channels: u16,
}

#[cfg(target_arch = "wasm32")]
pub fn available_output_devices() -> Vec<AudioDeviceInfo> {
    Vec::new()
}

#[cfg(not(target_arch = "wasm32"))]
pub fn available_output_devices() -> Vec<AudioDeviceInfo> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let Ok(devices) = cpal::default_host().output_devices() else { return Vec::new() };
    devices
        .filter_map(|d| {
            #[allow(deprecated)]
            let name = d.name().ok()?;
            let channels =
                d.supported_output_configs().ok()?.filter(|c| c.sample_format() == cpal::SampleFormat::F32).map(|c| c.channels()).max().unwrap_or(0);
            (channels > 0).then_some(AudioDeviceInfo { name, channels })
        })
        .collect()
}

#[cfg(test)]
mod sculpt_core22_audio_tests {
    use super::*;
    #[test]
    fn defaults_do_not_force_a_device() {
        let p = AudioSettings::default();
        assert!(p.output_uid.is_empty());
        assert_eq!(p.sample_rate, 0);
    }
}
