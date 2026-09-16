//! First-order ambisonic (FOA) spatial monitoring: the open path to game-audio SFX.
//!
//! Teaching note: placing a sound in 3D is two small linear maps. *Encode*
//! projects a mono/stereo source plus `azimuth`/`elevation`/`gain` onto the
//! four FOA channels `WXYZ` (the soundfield); *decode* projects `WXYZ` back
//! to a stereo pair for headphone monitoring. Both maps are pure functions
//! of samples — no state, no allocation inside the block fns — so the same
//! code serves the offline renderer, the realtime thread, and the mixer
//! monitor path.
//!
//! Dolby Atmos is proprietary/licensed and out of scope here; Ambisonics
//! (Gerzon, fully open) is the buildable path, and it fits game-audio SFX:
//! bake or place a one-shot in the field, monitor the placement on
//! headphones, ship the `WXYZ` (or the stereo monitor) with the bank.
//!
//! Conventions (pinned by the tests below):
//!
//! - Device params carry **degrees**: `azimuth` in `-180..=180` (`0` =
//!   front-center, positive = left, i.e. counter-clockwise seen from above),
//!   `elevation` in `-90..=90` (`0` = horizon, `+90` = zenith), `gain`
//!   linear in `0..=4` (the shared [`crate::devices::kernel::GAIN_PARAM`]).
//!   The DSP fns take **radians**.
//! - Direction unit vector: `front = cos(el)*cos(az)`, `left =
//!   cos(el)*sin(az)`, `up = sin(el)`.
//! - Normalization is unit (N3D-style): `W = s*g`, `X = s*g*front`, `Y =
//!   s*g*left`, `Z = s*g*up`. The omni channel carries the unattenuated
//!   signal, so a center-front source decodes loud and clear. This is a
//!   documented local convention, not an AmbiX/SN3D compliance claim
//!   (AmbiX scales `W` by `1/sqrt(2)` — readers compensating for that
//!   should scale `W` on import/export).
//! - Stereo decode is a virtual-cardioid, **HRTF-free** monitor matrix:
//!   `L = 0.5*(W + 0.5*X + Y)`, `R = 0.5*(W + 0.5*X - Y)`. Hence `L+R = W +
//!   X/2` (mono-compatible: front sources render hotter, rear quieter) and
//!   `L-R = Y` (left-positive difference). Height (`Z`) has no stereo
//!   image, so it folds to center through `W` only. There is no ITD/IID
//!   personalization and no head tracking — this is a placement monitor,
//!   unsuitable for critical binaural QA.
//!
//! Per-track params ride the existing frozen [`Node`](crate::model::Node)
//! shape (a `Spatial` device node holding `azimuth`/`elevation`/`gain`,
//! edited through ordinary [`OpKind::ParamSet`](crate::model::OpKind) ops —
//! see [`spatial_op_payloads`); no schema change. The mono-chain insert gain
//! ([`insert_gain`]) is the `(L+R)` projection of the field, so a `Spatial`
//! device in a mono insert chain holds the omni-plus-front energy while the
//! full `WXYZ` lives in the mixer monitor path
//! ([`crate::mixer::spatial_monitor_stereo`]).

use crate::model::Node;

/// `azimuth` param id in degrees (-180..=180, default 0 = front-center).
pub const AZIMUTH_PARAM: &str = "azimuth";
/// `elevation` param id in degrees (-90..=90, default 0 = horizon).
pub const ELEVATION_PARAM: &str = "elevation";

/// Default azimuth in degrees (front-center).
pub const DEFAULT_AZIMUTH_DEG: f64 = 0.0;
/// Default elevation in degrees (horizon).
pub const DEFAULT_ELEVATION_DEG: f64 = 0.0;
/// Default spatial gain (unity).
pub const DEFAULT_SPATIAL_GAIN: f64 = 1.0;

/// Errors from the ambisonic block maps.
#[derive(Debug, Clone, PartialEq)]
pub enum SpatialError {
    /// Channel slices disagree in length (outputs must match inputs).
    BadBlock(String),
}

impl std::fmt::Display for SpatialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadBlock(m) => write!(f, "bad spatial block: {m}"),
        }
    }
}

impl std::error::Error for SpatialError {}

/// One track's placement in the field, read from its `Spatial` device node
/// (see [`read_spatial_params`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpatialParams {
    pub azimuth_deg: f64,
    pub elevation_deg: f64,
    pub gain: f64,
}

impl Default for SpatialParams {
    fn default() -> Self {
        Self {
            azimuth_deg: DEFAULT_AZIMUTH_DEG,
            elevation_deg: DEFAULT_ELEVATION_DEG,
            gain: DEFAULT_SPATIAL_GAIN,
        }
    }
}

/// Direction unit vector `(front, left, up)` for radian angles.
pub fn direction(azimuth_rad: f64, elevation_rad: f64) -> [f64; 3] {
    let (saz, caz) = azimuth_rad.sin_cos();
    let (sel, cel) = elevation_rad.sin_cos();
    [cel * caz, cel * saz, sel]
}

/// Encode one mono sample into FOA `WXYZ` (`[w, x, y, z]`).
pub fn encode_mono(sample: f32, azimuth_rad: f64, elevation_rad: f64, gain: f64) -> [f32; 4] {
    let [front, left, up] = direction(azimuth_rad, elevation_rad);
    let s = sample as f64 * gain;
    [s as f32, (s * front) as f32, (s * left) as f32, (s * up) as f32]
}

/// Encode one stereo sample into FOA `WXYZ`.
///
/// `L` encodes at `azimuth - spread/2`, `R` at `azimuth + spread/2` (both at
/// `elevation`), and the pair averages — so `spread == 0` collapses exactly
/// to [`encode_mono`] of the mid `(l+r)/2`, and a mono source (`l == r`)
/// with zero spread is byte-identical to its mono encoding.
pub fn encode_stereo(
    l: f32,
    r: f32,
    azimuth_rad: f64,
    elevation_rad: f64,
    gain: f64,
    spread_rad: f64,
) -> [f32; 4] {
    let a = encode_mono(l, azimuth_rad - spread_rad / 2.0, elevation_rad, gain);
    let b = encode_mono(r, azimuth_rad + spread_rad / 2.0, elevation_rad, gain);
    [
        0.5 * (a[0] + b[0]),
        0.5 * (a[1] + b[1]),
        0.5 * (a[2] + b[2]),
        0.5 * (a[3] + b[3]),
    ]
}

/// Decode one FOA `WXYZ` frame to a stereo monitor pair `[l, r]`.
///
/// Virtual-cardioid, HRTF-free: `L = 0.5*(W + 0.5*X + Y)`, `R = 0.5*(W +
/// 0.5*X - Y)`. Not a binaural HRTF render — see the module docs.
pub fn decode_stereo(wxyz: [f32; 4]) -> [f32; 2] {
    let (w, x, y) = (wxyz[0], wxyz[1], wxyz[2]);
    [0.5 * (w + 0.5 * x + y), 0.5 * (w + 0.5 * x - y)]
}

/// Mono-chain insert gain holding the field's `(L+R)` projection:
/// `gain * (1 + front/2)` with degree inputs clamped to their param ranges.
/// Front-center passes hot (`1.5*g`), rear renders quiet (`0.5*g`), sides
/// pass unity — the omni-plus-front energy a mono insert can honestly keep.
pub fn insert_gain(azimuth_deg: f64, elevation_deg: f64, gain: f64) -> f32 {
    let az = azimuth_deg.clamp(-180.0, 180.0).to_radians();
    let el = elevation_deg.clamp(-90.0, 90.0).to_radians();
    let [front, _, _] = direction(az, el);
    (gain * (1.0 + front / 2.0)) as f32
}

fn check_block(name: &str, len: usize, other: usize) -> Result<(), SpatialError> {
    if len != other {
        return Err(SpatialError::BadBlock(format!(
            "{name}: {len} frames != {other} frames"
        )));
    }
    Ok(())
}

/// Encode a mono block into four channel blocks. All five slices must agree
/// in length (empty blocks are a legal no-op). No allocation.
pub fn encode_block_mono(
    input: &[f32],
    azimuth_rad: f64,
    elevation_rad: f64,
    gain: f64,
    w: &mut [f32],
    x: &mut [f32],
    y: &mut [f32],
    z: &mut [f32],
) -> Result<(), SpatialError> {
    let n = input.len();
    check_block("w", n, w.len())?;
    check_block("x", n, x.len())?;
    check_block("y", n, y.len())?;
    check_block("z", n, z.len())?;
    let [front, left, up] = direction(azimuth_rad, elevation_rad);
    for i in 0..n {
        let s = input[i] as f64 * gain;
        w[i] = s as f32;
        x[i] = (s * front) as f32;
        y[i] = (s * left) as f32;
        z[i] = (s * up) as f32;
    }
    Ok(())
}

/// Decode `WXYZ` blocks to a stereo monitor pair. All six slices must agree
/// in length (empty blocks are a legal no-op). No allocation.
pub fn decode_block_stereo(
    w: &[f32],
    x: &[f32],
    y: &[f32],
    z: &[f32],
    left: &mut [f32],
    right: &mut [f32],
) -> Result<(), SpatialError> {
    let n = w.len();
    check_block("x", n, x.len())?;
    check_block("y", n, y.len())?;
    check_block("z", n, z.len())?;
    check_block("left", n, left.len())?;
    check_block("right", n, right.len())?;
    let _ = z;
    for i in 0..n {
        let [l, r] = decode_stereo([w[i], x[i], y[i], z[i]]);
        left[i] = l;
        right[i] = r;
    }
    Ok(())
}

fn param_of(node: &Node, id: &str, default: f64) -> f64 {
    node.params.iter().find(|p| p.id == id).map(|p| p.value).unwrap_or(default)
}

/// Read a device node's spatial params (missing = defaults: front-center,
/// horizon, unity — the same tolerance the chain's wet/dry readers show).
pub fn read_spatial_params(node: &Node) -> SpatialParams {
    SpatialParams {
        azimuth_deg: param_of(node, AZIMUTH_PARAM, DEFAULT_AZIMUTH_DEG),
        elevation_deg: param_of(node, ELEVATION_PARAM, DEFAULT_ELEVATION_DEG),
        gain: param_of(
            node,
            crate::devices::kernel::GAIN_PARAM,
            DEFAULT_SPATIAL_GAIN,
        ),
    }
}

/// Expand a spatial placement to undoable [`OpKind::ParamSet`](crate::model::OpKind)
/// `(target, value_json)` pairs against `device:{param}` addresses, mirroring
/// [`crate::devices::presets::preset_op_payloads`]: every payload parses as a
/// bare number on the engine's `ParamSet` path.
pub fn spatial_op_payloads(
    device_id: &str,
    params: &SpatialParams,
) -> Vec<(String, String)> {
    [
        (AZIMUTH_PARAM, params.azimuth_deg),
        (ELEVATION_PARAM, params.elevation_deg),
        (crate::devices::kernel::GAIN_PARAM, params.gain),
    ]
    .iter()
    .map(|(param, value)| (format!("{device_id}:{param}"), format!("{value}")))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::class::{instantiate, DeviceClass};

    const DEG: f64 = std::f64::consts::PI / 180.0;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn encode_mono_pins_the_cardinal_points() {
        // Front-center: omni + full front, no side, no height.
        assert_eq!(encode_mono(1.0, 0.0, 0.0, 1.0), [1.0, 1.0, 0.0, 0.0]);
        // Hard left (+90 azimuth): omni + full left.
        let [w, x, y, z] = encode_mono(1.0, 90.0 * DEG, 0.0, 1.0);
        assert!(approx(w, 1.0) && approx(x, 0.0) && approx(y, 1.0) && approx(z, 0.0));
        // Rear: omni + negative front.
        let [w, x, y, z] = encode_mono(1.0, 180.0 * DEG, 0.0, 1.0);
        assert!(approx(w, 1.0) && approx(x, -1.0) && approx(y, 0.0) && approx(z, 0.0));
        // Zenith: omni + full up.
        let [w, x, y, z] = encode_mono(1.0, 0.0, 90.0 * DEG, 1.0);
        assert!(approx(w, 1.0) && approx(x, 0.0) && approx(y, 0.0) && approx(z, 1.0));
        // Gain scales every channel, including silence staying silent.
        assert_eq!(encode_mono(0.5, 0.0, 0.0, 2.0), [1.0, 1.0, 0.0, 0.0]);
        assert_eq!(encode_mono(0.0, 33.0 * DEG, 12.0 * DEG, 4.0), [0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn decode_stereo_is_exact_and_places_sources() {
        // Front-center decodes to dual-mono 0.75 (centered, hot).
        assert_eq!(decode_stereo([1.0, 1.0, 0.0, 0.0]), [0.75, 0.75]);
        // Hard left decodes to full-left (right silent).
        assert_eq!(decode_stereo([1.0, 0.0, 1.0, 0.0]), [1.0, 0.0]);
        // Hard right mirrors it.
        assert_eq!(decode_stereo([1.0, 0.0, -1.0, 0.0]), [0.0, 1.0]);
        // Rear sits center but quiet (cardioid monitor, not a failure).
        assert_eq!(decode_stereo([1.0, -1.0, 0.0, 0.0]), [0.25, 0.25]);
        // Silence stays silent.
        assert_eq!(decode_stereo([0.0, 0.0, 0.0, 0.0]), [0.0, 0.0]);
    }

    #[test]
    fn roundtrip_holds_sum_and_difference_identities() {
        // For any placement: L+R == W + X/2 and L-R == Y, exactly.
        for (az_deg, el_deg) in [
            (0.0, 0.0),
            (90.0, 0.0),
            (-45.0, 20.0),
            (180.0, -30.0),
            (30.0, 80.0),
        ] {
            let wxyz = encode_mono(0.7, az_deg * DEG, el_deg * DEG, 1.5);
            let [l, r] = decode_stereo(wxyz);
            assert!(
                approx(l + r, wxyz[0] + wxyz[1] / 2.0),
                "sum identity at ({az_deg}, {el_deg})"
            );
            assert!(approx(l - r, wxyz[2]), "diff identity at ({az_deg}, {el_deg})");
        }
        // Front renders hotter than rear (the monitor's cardioid property).
        let front = decode_stereo(encode_mono(1.0, 0.0, 0.0, 1.0));
        let rear = decode_stereo(encode_mono(1.0, 180.0 * DEG, 0.0, 1.0));
        assert!(front[0] + front[1] > rear[0] + rear[1]);
        // Height folds to center (equal L/R), never to one side.
        let top = decode_stereo(encode_mono(1.0, 0.0, 90.0 * DEG, 1.0));
        assert_eq!(top[0], top[1]);
    }

    #[test]
    fn stereo_encode_collapses_to_mono_mid_at_zero_spread() {
        // Zero spread: exactly the mono encoding of the mid.
        for (l, r) in [(1.0, 0.0), (0.5, -0.25), (-1.0, -1.0)] {
            let got = encode_stereo(l, r, 30.0 * DEG, 10.0 * DEG, 2.0, 0.0);
            assert_eq!(got, encode_mono((l + r) / 2.0, 30.0 * DEG, 10.0 * DEG, 2.0));
        }
        // Mono source, zero spread: identical to the mono path.
        assert_eq!(
            encode_stereo(0.8, 0.8, 0.0, 0.0, 1.0, 0.0),
            encode_mono(0.8, 0.0, 0.0, 1.0)
        );
        // Spread steers L/R apart: L lands left of R.
        let [_, _, y, _] = encode_stereo(1.0, 1.0, 0.0, 0.0, 1.0, 60.0 * DEG);
        let [_, _, yl, _] = encode_mono(1.0, -30.0 * DEG, 0.0, 1.0);
        let [_, _, yr, _] = encode_mono(1.0, 30.0 * DEG, 0.0, 1.0);
        assert!(approx(y, 0.5 * (yl + yr)));
    }

    #[test]
    fn block_maps_match_scalar_maps_and_reject_ragged_blocks() {
        let input = vec![1.0f32, 0.5, -0.25, 0.0];
        let (mut w, mut x, mut y, mut z) = (
            vec![0.0; 4],
            vec![0.0; 4],
            vec![0.0; 4],
            vec![0.0; 4],
        );
        encode_block_mono(&input, 45.0 * DEG, 0.0, 1.0, &mut w, &mut x, &mut y, &mut z)
            .expect("encode");
        for i in 0..4 {
            assert_eq!([w[i], x[i], y[i], z[i]], encode_mono(input[i], 45.0 * DEG, 0.0, 1.0));
        }
        let (mut l, mut r) = (vec![0.0; 4], vec![0.0; 4]);
        decode_block_stereo(&w, &x, &y, &z, &mut l, &mut r).expect("decode");
        for i in 0..4 {
            assert_eq!([l[i], r[i]], decode_stereo([w[i], x[i], y[i], z[i]]));
        }
        // Ragged blocks error; empty blocks are a legal no-op.
        let mut short = vec![0.0; 3];
        assert!(matches!(
            encode_block_mono(&input, 0.0, 0.0, 1.0, &mut w, &mut x, &mut y, &mut short),
            Err(SpatialError::BadBlock(_))
        ));
        assert!(matches!(
            decode_block_stereo(&w, &x, &y, &z, &mut l, &mut short),
            Err(SpatialError::BadBlock(_))
        ));
        let (mut e0, mut e1, mut e2, mut e3): (
            Vec<f32>,
            Vec<f32>,
            Vec<f32>,
            Vec<f32>,
        ) = (vec![], vec![], vec![], vec![]);
        encode_block_mono(&[], 0.0, 0.0, 1.0, &mut e0, &mut e1, &mut e2, &mut e3)
            .expect("empty encode ok");
    }

    #[test]
    fn insert_gain_pins_front_rear_and_clamps_degrees() {
        assert!(approx(insert_gain(0.0, 0.0, 1.0), 1.5));
        assert!(approx(insert_gain(180.0, 0.0, 1.0), 0.5));
        assert!(approx(insert_gain(90.0, 0.0, 1.0), 1.0));
        assert!(approx(insert_gain(0.0, 0.0, 2.0), 3.0));
        // Out-of-range degrees clamp to the param ranges, never NaN:
        // 999 azimuth clamps to 180 (rear, 0.5x); 999 elevation clamps to
        // the zenith, whose front component is 0 (unity).
        assert!(approx(insert_gain(999.0, 0.0, 1.0), 0.5));
        assert!(approx(insert_gain(0.0, 999.0, 1.0), 1.0));
    }

    #[test]
    fn params_read_defaults_and_emit_param_set_payloads() {
        let node = instantiate(DeviceClass::Spatial, "sp1", "Place");
        let p = read_spatial_params(&node);
        assert_eq!(
            p,
            SpatialParams {
                azimuth_deg: 0.0,
                elevation_deg: 0.0,
                gain: 1.0,
            }
        );
        // A node that predates this track (no spatial params) reads defaults.
        let bare = Node {
            id: "old".to_string(),
            kind: crate::model::NodeKind::Device,
            name: "Old".to_string(),
            params: vec![],
        };
        assert_eq!(read_spatial_params(&bare), SpatialParams::default());
        // Payloads are engine-ready ParamSet pairs parsing as bare numbers.
        let payloads = spatial_op_payloads(
            "sp1",
            &SpatialParams {
                azimuth_deg: -30.0,
                elevation_deg: 10.0,
                gain: 1.5,
            },
        );
        assert_eq!(
            payloads,
            vec![
                ("sp1:azimuth".to_string(), "-30".to_string()),
                ("sp1:elevation".to_string(), "10".to_string()),
                ("sp1:gain".to_string(), "1.5".to_string()),
            ]
        );
        for (_, value_json) in &payloads {
            let v: f64 = value_json.parse().expect("bare number");
            assert!(v.is_finite());
        }
    }
}
