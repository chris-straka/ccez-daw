//! Device classes: what kind of DSP a frozen `Node` is.
//!
//! Teaching note: the frozen [`Node`](crate::model::Node) has no "type"
//! field — just `id`, `kind`, `name`, and `params` — and Track D may not
//! add one (that would be a schema change plus a migration note). So the
//! class rides *inside* the frozen `Param` shape as a numeric code in the
//! [`DEVICE_CLASS_PARAM`] param, exactly the way Track C's oversampling
//! flag rides in `"oversampling"`. [`classify`] reads it; [`instantiate`]
//! writes it. Nodes that predate this track (hand-built, or owned by a
//! plugin host) carry no code and classify as [`DeviceClass::Foreign`],
//! which the rack renderer treats as pass-through — structure survives
//! nodes this track does not understand.

use crate::model::{Node, NodeKind, Param};

use super::kernel::{
    CUTOFF_PARAM, DELAY_SAMPLES_PARAM, DRIVE_PARAM, FEEDBACK_PARAM, GAIN_PARAM,
};
use super::DeviceError;

/// Numeric class tag param. Missing or unrecognized = [`DeviceClass::Foreign`].
pub const DEVICE_CLASS_PARAM: &str = "device_class";

/// Class codes stored in [`DEVICE_CLASS_PARAM`]. Append-only: renumbering
/// a code would orphan saved projects, so new classes take the next int.
pub const CLASS_GAIN: f64 = 1.0;
pub const CLASS_LOWPASS: f64 = 2.0;
pub const CLASS_HIGHPASS: f64 = 3.0;
pub const CLASS_DELAY: f64 = 4.0;
pub const CLASS_DISTORTION: f64 = 5.0;
pub const CLASS_CONTAINER: f64 = 6.0;

/// What DSP a device node carries. `Container` holds structure (children
/// live in the [`crate::devices::Rack`] sidecar), not samples. `Foreign`
/// is any node without a recognized code — rendered as pass-through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceClass {
    Gain,
    Lowpass,
    Highpass,
    Delay,
    Distortion,
    Container,
    Foreign,
}

impl DeviceClass {
    pub fn code(self) -> f64 {
        match self {
            Self::Gain => CLASS_GAIN,
            Self::Lowpass => CLASS_LOWPASS,
            Self::Highpass => CLASS_HIGHPASS,
            Self::Delay => CLASS_DELAY,
            Self::Distortion => CLASS_DISTORTION,
            Self::Container => CLASS_CONTAINER,
            Self::Foreign => 0.0,
        }
    }

    pub(crate) fn from_code(code: f64) -> Self {
        if code == CLASS_GAIN {
            Self::Gain
        } else if code == CLASS_LOWPASS {
            Self::Lowpass
        } else if code == CLASS_HIGHPASS {
            Self::Highpass
        } else if code == CLASS_DELAY {
            Self::Delay
        } else if code == CLASS_DISTORTION {
            Self::Distortion
        } else if code == CLASS_CONTAINER {
            Self::Container
        } else {
            Self::Foreign
        }
    }
}

/// Read a node's class from its [`DEVICE_CLASS_PARAM`] code.
/// Never errors: unknown is [`DeviceClass::Foreign`], by design.
pub fn classify(node: &Node) -> DeviceClass {
    match node.params.iter().find(|p| p.id == DEVICE_CLASS_PARAM) {
        Some(p) => DeviceClass::from_code(p.value),
        None => DeviceClass::Foreign,
    }
}

fn param(id: &str, label: &str, value: f64, min: f64, max: f64, unit: &str) -> Param {
    Param {
        id: id.to_string(),
        label: label.to_string(),
        value: value.clamp(min, max),
        min,
        max,
        default: value.clamp(min, max),
        unit: unit.to_string(),
    }
}

fn class_tag(class: DeviceClass) -> Param {
    Param {
        id: DEVICE_CLASS_PARAM.to_string(),
        label: "Device class".to_string(),
        value: class.code(),
        min: 0.0,
        max: 255.0,
        default: class.code(),
        unit: String::new(),
    }
}

/// Default params for a class (without the class tag; [`instantiate`]
/// prepends it). The single place reusable-device defaults live.
pub fn default_params(class: DeviceClass) -> Vec<Param> {
    match class {
        DeviceClass::Gain => vec![param(GAIN_PARAM, "Gain", 1.0, 0.0, 4.0, "x")],
        DeviceClass::Lowpass => vec![param(CUTOFF_PARAM, "Cutoff", 1000.0, 20.0, 20000.0, "Hz")],
        DeviceClass::Highpass => vec![param(CUTOFF_PARAM, "Cutoff", 1000.0, 20.0, 20000.0, "Hz")],
        DeviceClass::Delay => vec![
            param(DELAY_SAMPLES_PARAM, "Delay", 0.0, 0.0, 48000.0, "samples"),
            param(FEEDBACK_PARAM, "Feedback", 0.0, 0.0, 0.95, ""),
        ],
        DeviceClass::Distortion => vec![param(DRIVE_PARAM, "Drive", 1.0, 0.0, 10.0, "")],
        DeviceClass::Container => vec![],
        DeviceClass::Foreign => vec![],
    }
}

/// Build a fresh frozen device node of `class`: the class tag plus the
/// class defaults. This is the reusable-device factory — every preset,
/// container child, and split branch starts here.
pub fn instantiate(class: DeviceClass, id: &str, name: &str) -> Node {
    let mut params = vec![class_tag(class)];
    params.extend(default_params(class));
    Node {
        id: id.to_string(),
        kind: NodeKind::Device,
        name: name.to_string(),
        params,
    }
}

/// Read one param with a fallback default (missing = default, the same
/// tolerance the chain's wet/dry readers show).
pub fn param_value(node: &Node, id: &str, default: f64) -> f64 {
    node.params.iter().find(|p| p.id == id).map(|p| p.value).unwrap_or(default)
}

/// Set one param, clamped to its `[min, max]`. Unknown ids are
/// [`DeviceError::UnknownParam`] — typos must surface, not vanish.
pub fn set_param_value(node: &mut Node, id: &str, value: f64) -> Result<(), DeviceError> {
    match node.params.iter_mut().find(|p| p.id == id) {
        Some(p) => {
            p.value = value.clamp(p.min, p.max);
            Ok(())
        }
        None => Err(DeviceError::UnknownParam(format!("{}:{}", node.id, id))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instantiate_classify_round_trip() {
        for class in [
            DeviceClass::Gain,
            DeviceClass::Lowpass,
            DeviceClass::Highpass,
            DeviceClass::Delay,
            DeviceClass::Distortion,
            DeviceClass::Container,
        ] {
            let node = instantiate(class, "d", "D");
            assert_eq!(classify(&node), class, "{class:?}");
            // The tag survives the frozen model JSON (it is just a Param).
            let json = serde_json::to_string(&node).expect("serialize");
            let back: Node = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(classify(&back), class);
        }
    }

    #[test]
    fn legacy_nodes_are_foreign_not_errors() {
        let node = Node {
            id: "old".to_string(),
            kind: NodeKind::Device,
            name: "Old".to_string(),
            params: vec![param(GAIN_PARAM, "Gain", 2.0, 0.0, 4.0, "x")],
        };
        assert_eq!(classify(&node), DeviceClass::Foreign);
    }

    #[test]
    fn set_param_clamps_and_rejects_unknown_ids() {
        let mut node = instantiate(DeviceClass::Gain, "g", "G");
        set_param_value(&mut node, GAIN_PARAM, 99.0).expect("set");
        assert_eq!(param_value(&node, GAIN_PARAM, 0.0), 4.0);
        assert!(matches!(
            set_param_value(&mut node, "nope", 1.0),
            Err(DeviceError::UnknownParam(_))
        ));
        assert_eq!(param_value(&node, "missing", 7.0), 7.0);
    }
}
