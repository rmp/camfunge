//! JSON control protocol spoken over the service's WebSocket (`ws://127.0.0.1:<port>/`).
//!
//! Client → service: text frames holding a [`ClientMessage`].
//! Service → client: text frames holding a [`ServerMessage`]; when a client has asked for a preview,
//! binary frames each holding one JPEG of the output.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReactTo {
    Movement,
    Colour,
    Sound,
}

/// Everything the GUI can change. Sent whole by the service; clients send a [`ConfigPatch`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    /// Camera id from [`CameraInfo::id`]; `None` means the first real camera.
    pub camera: Option<String>,
    /// Index into the hello message's `filters`.
    pub filter: usize,
    /// 0..1
    pub intensity: f32,
    /// 0..3
    pub speed: f32,
    pub react_to: ReactTo,
    /// 0..1
    pub reactivity: f32,
    /// Advance to the next filter every [`crate::engine::AUTO_CYCLE_SECS`].
    pub auto_cycle: bool,
    /// Wave in the top-left / top-right of the picture to change filter.
    pub gestures: bool,
    /// Selfie-mirror the picture (the original app always did).
    pub mirror: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            camera: None,
            filter: 0,
            intensity: 0.6,
            speed: 1.0,
            react_to: ReactTo::Movement,
            reactivity: 0.5,
            auto_cycle: false,
            gestures: false,
            mirror: true,
        }
    }
}

/// A partial [`Config`]: absent fields are left alone. `camera: null` selects the default camera.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigPatch {
    #[serde(default, with = "double_option", skip_serializing_if = "Option::is_none")]
    pub camera: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intensity: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub react_to: Option<ReactTo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reactivity: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_cycle: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gestures: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mirror: Option<bool>,
}

mod double_option {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    pub fn serialize<S: Serializer>(v: &Option<Option<String>>, s: S) -> Result<S::Ok, S::Error> {
        v.as_ref().unwrap_or(&None).serialize(s)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<String>>, D::Error> {
        Option::<String>::deserialize(d).map(Some)
    }
}

fn finite_clamp(v: f32, lo: f32, hi: f32) -> Option<f32> {
    v.is_finite().then(|| v.clamp(lo, hi))
}

impl Config {
    /// Applies a patch, clamping numbers into range and wrapping the filter index. Non-finite numbers are ignored.
    pub fn apply(&mut self, p: &ConfigPatch, filter_count: usize) {
        if let Some(c) = &p.camera {
            self.camera = c.clone();
        }
        if let Some(f) = p.filter {
            self.filter = f % filter_count.max(1);
        }
        if let Some(v) = p.intensity.and_then(|v| finite_clamp(v, 0.0, 1.0)) {
            self.intensity = v;
        }
        if let Some(v) = p.speed.and_then(|v| finite_clamp(v, 0.0, 3.0)) {
            self.speed = v;
        }
        if let Some(v) = p.react_to {
            self.react_to = v;
        }
        if let Some(v) = p.reactivity.and_then(|v| finite_clamp(v, 0.0, 1.0)) {
            self.reactivity = v;
        }
        if let Some(v) = p.auto_cycle {
            self.auto_cycle = v;
        }
        if let Some(v) = p.gestures {
            self.gestures = v;
        }
        if let Some(v) = p.mirror {
            self.mirror = v;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ClientMessage {
    /// Change some settings.
    Set { config: ConfigPatch },
    /// Move `by` filters forward (negative: back), wrapping.
    Step { by: i64 },
    /// Re-capture the empty-room background used by the cloaking filters.
    GrabBackground,
    /// Start or stop JPEG preview frames to this client.
    Preview {
        enabled: bool,
        /// Longest edge of preview frames; defaults to 640.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_size: Option<u32>,
    },
    /// Re-list cameras (sends a fresh `hello`).
    Refresh,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterInfo {
    pub name: String,
    pub artist: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraInfo {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VirtualCameraInfo {
    /// `obs-macos`, `obs-windows`, `v4l2loopback` or `none`.
    pub backend: String,
    /// Name the virtual camera shows up as in other apps, if it is running.
    pub device: Option<String>,
    /// Why the virtual camera is not running, if it isn't.
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputInfo {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ServerMessage {
    /// Sent on connect and after `refresh`.
    Hello {
        version: String,
        filters: Vec<FilterInfo>,
        cameras: Vec<CameraInfo>,
        config: Config,
        virtual_camera: VirtualCameraInfo,
        output: OutputInfo,
        gpu: String,
    },
    /// The whole config, broadcast whenever anything changes it (any client, auto-cycle, gestures).
    Config { config: Config },
    /// A few times a second.
    Status {
        /// Smoothed 0..1 reaction level (drives the GUI meter).
        level: f32,
        /// Rendered frames per second over the last interval.
        fps: f32,
        /// Current input problem (no camera, no microphone…), if any.
        error: Option<String>,
    },
    /// A request could not be handled.
    Error { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(v: serde_json::Value) -> ClientMessage {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn set_patch_parses_partial_camel_case() {
        let m = parse(json!({ "type": "set", "config": { "reactTo": "sound", "autoCycle": true } }));
        let ClientMessage::Set { config } = m else { panic!() };
        assert_eq!(config.react_to, Some(ReactTo::Sound));
        assert_eq!(config.auto_cycle, Some(true));
        assert_eq!(config.filter, None);
        assert_eq!(config.camera, None);
    }

    #[test]
    fn camera_null_means_default_and_absent_means_unchanged() {
        let mut c = Config { camera: Some("1".into()), ..Config::default() };
        let ClientMessage::Set { config: absent } = parse(json!({ "type": "set", "config": {} })) else { panic!() };
        c.apply(&absent, 3);
        assert_eq!(c.camera.as_deref(), Some("1"));
        let ClientMessage::Set { config: null } = parse(json!({ "type": "set", "config": { "camera": null } })) else { panic!() };
        c.apply(&null, 3);
        assert_eq!(c.camera, None);
    }

    #[test]
    fn apply_clamps_wraps_and_ignores_nan() {
        let mut c = Config::default();
        c.apply(&ConfigPatch { filter: Some(7), intensity: Some(4.0), speed: Some(-1.0), reactivity: Some(f32::NAN), ..Default::default() }, 5);
        assert_eq!((c.filter, c.intensity, c.speed, c.reactivity), (2, 1.0, 0.0, 0.5));
    }

    #[test]
    fn unknown_fields_and_types_are_rejected() {
        assert!(serde_json::from_value::<ClientMessage>(json!({ "type": "set", "config": { "bogus": 1 } })).is_err());
        assert!(serde_json::from_value::<ClientMessage>(json!({ "type": "launchMissiles" })).is_err());
    }

    #[test]
    fn server_messages_are_tagged_camel_case() {
        let v = serde_json::to_value(ServerMessage::Status { level: 0.5, fps: 30.0, error: None }).unwrap();
        assert_eq!(v, json!({ "type": "status", "level": 0.5, "fps": 30.0, "error": null }));
        let v = serde_json::to_value(ServerMessage::Config { config: Config::default() }).unwrap();
        assert_eq!(v["config"]["reactTo"], "movement");
        assert_eq!(v["config"]["autoCycle"], false);
    }

    #[test]
    fn fields_inside_messages_are_camel_case_too() {
        let ClientMessage::Preview { max_size, .. } = parse(json!({ "type": "preview", "enabled": true, "maxSize": 320 })) else { panic!() };
        assert_eq!(max_size, Some(320));
        let hello = ServerMessage::Hello {
            version: "0".into(),
            filters: vec![],
            cameras: vec![],
            config: Config::default(),
            virtual_camera: VirtualCameraInfo { backend: "none".into(), device: None, error: None },
            output: OutputInfo { width: 2, height: 2, fps: 30 },
            gpu: "test".into(),
        };
        let v = serde_json::to_value(hello).unwrap();
        assert_eq!(v["virtualCamera"]["backend"], "none");
        assert!(v.get("virtual_camera").is_none());
    }

    #[test]
    fn simple_commands_round_trip() {
        for m in [
            ClientMessage::Step { by: -1 },
            ClientMessage::GrabBackground,
            ClientMessage::Preview { enabled: true, max_size: Some(320) },
            ClientMessage::Refresh,
        ] {
            let s = serde_json::to_string(&m).unwrap();
            assert_eq!(serde_json::from_str::<ClientMessage>(&s).unwrap(), m);
        }
        assert_eq!(parse(json!({ "type": "grabBackground" })), ClientMessage::GrabBackground);
    }
}
