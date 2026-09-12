//! Parameter bridge: the universal `node:param` address, down to the guest.
//!
//! Teaching note: the DAW has exactly one param-address grammar —
//! [`crate::engine`] applies `ParamSet` ops whose target is
//! `node:param` (see `contracts/op-log-format.md`: "param address
//! (`node:param`)"). A WASM device must speak that same grammar, not
//! invent a second one, or automation, the op log, and macros could never
//! drive it. This module is that glue, in two directions:
//!
//! - [`parse_address`] splits `"dev_dly:feedback"` into `("dev_dly",
//!   "feedback")` with the engine's `split_once(':')` semantics (so an
//!   address with no colon, or an empty side, is [`Error::BadAddress`];
//!   extra colons stay in the param half, exactly as the engine leaves
//!   them — one grammar, one behavior).
//! - [`sync_from_node`] hydrates a [`GuestDelay`](super::guest::GuestDelay)
//!   from a frozen [`Node`](crate::model::Node): known ids apply with
//!   guest clamps, unknown ids are skipped — the same tolerance the
//!   native [`crate::dsp::Delay::from_node`] shows (`let _ =`), because a
//!   bulk load must survive params the guest does not know (wet/dry/mix
//!   convention params, future additions), while a single addressed set
//!   must refuse typos (see [`crate::wasmdevices::sandbox`]).

use crate::model::Node;

use super::guest::GuestDelay;
use super::Error;

/// Split a universal param address into `(node_id, param_id)`.
///
/// Mirrors the engine's `ParamSet` handling: exactly one grammar, shared
/// by ops, automation, and this bridge.
pub fn parse_address(target: &str) -> Result<(&str, &str), Error> {
    match target.split_once(':') {
        Some((node, param)) if !node.is_empty() && !param.is_empty() => Ok((node, param)),
        _ => Err(Error::BadAddress(target.to_string())),
    }
}

/// Hydrate `guest` from a frozen device [`Node`]: every known param
/// applies (with guest clamps); unknown ids are skipped, not errors —
/// the bulk-load tolerance the native `from_node` shows. Returns the
/// number of params applied.
pub fn sync_from_node(guest: &mut GuestDelay, node: &Node) -> usize {
    let mut applied = 0;
    for p in &node.params {
        if guest.apply_param(&p.id, p.value).is_ok() {
            applied += 1;
        }
    }
    applied
}

/// The guest's canonical default node: same ids, ranges, and defaults as
/// the native [`crate::dsp::Delay::default_node`], so a project can swap
/// the native device for the sandboxed one with zero param edits.
pub fn default_node(id: &str, name: &str) -> Node {
    crate::dsp::Delay::default_node(id, name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_split_like_the_engine() {
        assert_eq!(parse_address("dev_dly:feedback").unwrap(), ("dev_dly", "feedback"));
        // Extra colons stay in the param half — `split_once`, like engine.
        assert_eq!(parse_address("d:mix:extra").unwrap(), ("d", "mix:extra"));
        assert!(matches!(parse_address("no-colon"), Err(Error::BadAddress(_))));
        assert!(matches!(parse_address(":mix"), Err(Error::BadAddress(_))));
        assert!(matches!(parse_address("d:"), Err(Error::BadAddress(_))));
    }

    #[test]
    fn bulk_sync_applies_known_and_skips_unknown() {
        let mut guest = GuestDelay::new(48_000.0);
        let mut node = default_node("dev_dly", "Sandboxed Delay");
        node.params.push(crate::model::Param {
            id: "wet".to_string(),
            label: "Wet".to_string(),
            value: 0.9,
            min: 0.0,
            max: 1.0,
            default: 1.0,
            unit: String::new(),
        });
        let applied = sync_from_node(&mut guest, &node);
        assert_eq!(applied, 3);
        assert_eq!(guest.mix, 0.3);
        assert_eq!(guest.time_ms, 375.0);
    }
}
