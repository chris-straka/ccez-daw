//! Rust-to-TypeScript emitter. Each struct/enum is declared once here with
//! its TypeScript type and its Zod v4 schema fragment side by side, so the
//! schema cannot drift from the interface by hand-edit.

use crate::ipc::{COMMANDS, EVENTS};
use crate::model::CONTRACT_VERSION;

pub struct Field {
    pub name: &'static str,
    pub ts: &'static str,
    pub zod: &'static str,
}

pub struct StructDef {
    pub name: &'static str,
    pub doc: &'static str,
    pub fields: &'static [Field],
}

pub struct EnumDef {
    pub name: &'static str,
    pub doc: &'static str,
    pub variants: &'static [&'static str],
}

const F_STRING: &str = "string";
const Z_STRING: &str = "z.string()";
const F_NUMBER: &str = "number";
const Z_NUMBER: &str = "z.number()";
const F_BOOLEAN: &str = "boolean";
const Z_BOOLEAN: &str = "z.boolean()";

macro_rules! f {
    ($name:literal, $ts:expr, $zod:expr) => {
        Field {
            name: $name,
            ts: $ts,
            zod: $zod,
        }
    };
}

pub const ENUMS: &[EnumDef] = &[
    EnumDef {
        name: "NodeKind",
        doc: "Addressable node discriminator: track, clip, device, modulator, or bus.",
        variants: &["Track", "Clip", "Device", "Modulator", "Bus"],
    },
    EnumDef {
        name: "EdgeKind",
        doc: "Routing-graph edge discriminator. The mixer is a view over Audio edges.",
        variants: &["Audio", "Midi", "Modulation", "Sidechain"],
    },
    EnumDef {
        name: "ClipKind",
        doc: "Clip payload discriminator.",
        variants: &["Audio", "Midi"],
    },
    EnumDef {
        name: "EngineState",
        doc: "Audio engine transport state.",
        variants: &["Stopped", "Playing", "Recording"],
    },
    EnumDef {
        name: "OpKind",
        doc: "Op-log entry discriminator (event-sourced project document).",
        variants: &[
            "TrackAdded",
            "ClipAdded",
            "ClipMoved",
            "ParamSet",
            "TempoSet",
            "UndoMarker",
        ],
    },
    // Track J (additive): browser library kinds. Existing entries untouched.
    EnumDef {
        name: "LibraryKind",
        doc: "Browsable library discriminator: one palette indexes all four.",
        variants: &["Sample", "Preset", "Plugin", "Project"],
    },
];

pub const STRUCTS: &[StructDef] = &[
    StructDef {
        name: "ParamAddress",
        doc: "Universal address of one automatable/modulatable parameter.",
        fields: &[
            f!("node", F_STRING, Z_STRING),
            f!("param", F_STRING, Z_STRING),
        ],
    },
    StructDef {
        name: "Param",
        doc: "One parameter with range metadata.",
        fields: &[
            f!("id", F_STRING, Z_STRING),
            f!("label", F_STRING, Z_STRING),
            f!("value", F_NUMBER, Z_NUMBER),
            f!("min", F_NUMBER, Z_NUMBER),
            f!("max", F_NUMBER, Z_NUMBER),
            f!("default", F_NUMBER, Z_NUMBER),
            f!("unit", F_STRING, Z_STRING),
        ],
    },
    StructDef {
        name: "Node",
        doc: "One addressable object: track, clip, device, modulator, or bus.",
        fields: &[
            f!("id", F_STRING, Z_STRING),
            f!("kind", "NodeKind", "NodeKindSchema"),
            f!("name", F_STRING, Z_STRING),
            f!("params", "Param[]", "z.array(ParamSchema)"),
        ],
    },
    StructDef {
        name: "Edge",
        doc: "One routing-graph edge (audio, MIDI, modulation, sidechain).",
        fields: &[
            f!("id", F_STRING, Z_STRING),
            f!("from_node", F_STRING, Z_STRING),
            f!("from_port", F_STRING, Z_STRING),
            f!("to_node", F_STRING, Z_STRING),
            f!("to_port", F_STRING, Z_STRING),
            f!("kind", "EdgeKind", "EdgeKindSchema"),
        ],
    },
    StructDef {
        name: "Clip",
        doc: "One timeline clip on a track.",
        fields: &[
            f!("id", F_STRING, Z_STRING),
            f!("track_id", F_STRING, Z_STRING),
            f!("name", F_STRING, Z_STRING),
            f!("start_beats", F_NUMBER, Z_NUMBER),
            f!("length_beats", F_NUMBER, Z_NUMBER),
            f!("kind", "ClipKind", "ClipKindSchema"),
            f!("source", F_STRING, Z_STRING),
        ],
    },
    StructDef {
        name: "Track",
        doc: "One track: mixer strip + clip lane over the universal model.",
        fields: &[
            f!("id", F_STRING, Z_STRING),
            f!("name", F_STRING, Z_STRING),
            f!("volume", F_NUMBER, Z_NUMBER),
            f!("pan", F_NUMBER, Z_NUMBER),
            f!("muted", F_BOOLEAN, Z_BOOLEAN),
            f!("solo", F_BOOLEAN, Z_BOOLEAN),
            f!("clip_ids", "string[]", "z.array(z.string())"),
            f!("device_ids", "string[]", "z.array(z.string())"),
        ],
    },
    StructDef {
        name: "AutomationPoint",
        doc: "One sample-accurate automation breakpoint.",
        fields: &[
            f!("beat", F_NUMBER, Z_NUMBER),
            f!("value", F_NUMBER, Z_NUMBER),
        ],
    },
    StructDef {
        name: "AutomationLane",
        doc: "Automation lane bound to one ParamAddress.",
        fields: &[
            f!("id", F_STRING, Z_STRING),
            f!("target", "ParamAddress", "ParamAddressSchema"),
            f!("points", "AutomationPoint[]", "z.array(AutomationPointSchema)"),
        ],
    },
    StructDef {
        name: "Project",
        doc: "The saved project document (v0).",
        fields: &[
            f!("schema_version", F_NUMBER, Z_NUMBER),
            f!("id", F_STRING, Z_STRING),
            f!("name", F_STRING, Z_STRING),
            f!("tempo", F_NUMBER, Z_NUMBER),
            f!("time_sig_num", F_NUMBER, Z_NUMBER),
            f!("time_sig_den", F_NUMBER, Z_NUMBER),
            f!("tracks", "Track[]", "z.array(TrackSchema)"),
            f!("clips", "Clip[]", "z.array(ClipSchema)"),
            f!("devices", "Node[]", "z.array(NodeSchema)"),
            f!("routing", "Edge[]", "z.array(EdgeSchema)"),
            f!("automation", "AutomationLane[]", "z.array(AutomationLaneSchema)"),
        ],
    },
    StructDef {
        name: "Op",
        doc: "One event-sourced op-log entry.",
        fields: &[
            f!("seq", F_NUMBER, Z_NUMBER),
            f!("actor", F_STRING, Z_STRING),
            f!("kind", "OpKind", "OpKindSchema"),
            f!("target", F_STRING, Z_STRING),
            f!("value_json", F_STRING, Z_STRING),
        ],
    },
    // Track J (additive): browser library shapes. Appended last so every
    // previously emitted line renders byte-identically.
    StructDef {
        name: "LibraryItem",
        doc: "One browsable library entry: sample, preset, plugin, or project.",
        fields: &[
            f!("id", F_STRING, Z_STRING),
            f!("kind", "LibraryKind", "LibraryKindSchema"),
            f!("name", F_STRING, Z_STRING),
            f!("tags", "string[]", "z.array(z.string())"),
            f!("text", F_STRING, Z_STRING),
        ],
    },
    StructDef {
        name: "LibraryHit",
        doc: "One ranked similarity-search hit: item id plus cosine score.",
        fields: &[
            f!("id", F_STRING, Z_STRING),
            f!("score", F_NUMBER, Z_NUMBER),
        ],
    },
];

fn header() -> String {
    format!(
        "// GENERATED by `bun run typegen` from the ccez-core Rust crate.\n// Contract {CONTRACT_VERSION}. DO NOT EDIT BY HAND.\n"
    )
}

/// Render `ui/src/generated/project.ts`: Zod v4 schemas + inferred TS types.
pub fn render_project_ts() -> String {
    let mut out = header();
    out.push_str("import { z } from \"zod\";\n\n");
    out.push_str(&format!(
        "export const CONTRACT_VERSION = \"{CONTRACT_VERSION}\" as const;\n\
         export const SCHEMA_VERSION = 0 as const;\n\n"
    ));
    for e in ENUMS {
        out.push_str(&format!("/** {} */\n", e.doc));
        let variants: Vec<String> =
            e.variants.iter().map(|v| format!("\"{v}\"")).collect();
        out.push_str(&format!(
            "export const {0}Schema = z.enum([{1}]);\n",
            e.name,
            variants.join(", ")
        ));
        out.push_str(&format!(
            "export type {0} = z.infer<typeof {0}Schema>;\n\n",
            e.name
        ));
    }
    for s in STRUCTS {
        out.push_str(&format!("/** {} */\n", s.doc));
        out.push_str(&format!("export const {}Schema = z.object({{\n", s.name));
        for fl in s.fields {
            out.push_str(&format!("  {}: {},\n", fl.name, fl.zod));
        }
        out.push_str("});\n");
        out.push_str(&format!(
            "export type {0} = z.infer<typeof {0}Schema>;\n\n",
            s.name
        ));
    }
    out
}

/// Render `ui/src/generated/ipc.ts`: typed `invoke` wrappers + event names.
pub fn render_ipc_ts() -> String {
    let mut out = header();
    out.push_str(
        "import { invoke } from \"@tauri-apps/api/core\";\n\
         import type { Clip, EngineState, Op, ParamAddress, Project } from \"./project\";\n\n",
    );
    for c in COMMANDS {
        out.push_str(&format!("/** {} */\n", c.doc));
        out.push_str(&format!(
            "export function {}(args: {}): Promise<{}> {{\n  return invoke<{}>(\"{}\", args as unknown as Record<string, unknown>);\n}}\n\n",
            c.name, c.args_ts, c.returns_ts, c.returns_ts, c.name
        ));
    }
    out.push_str("export const COMMAND_NAMES = [\n");
    for c in COMMANDS {
        out.push_str(&format!("  \"{}\",\n", c.name));
    }
    out.push_str("] as const;\n\n");
    out.push_str("export const EVENTS = {\n");
    for e in EVENTS {
        out.push_str(&format!("  /** {} */\n  {}: \"{}\",\n", e.doc, e.name, e.name));
    }
    out.push_str("} as const;\n\nexport type EventName = keyof typeof EVENTS;\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_ipc_type_reference_resolves_to_an_emitted_type() {
        let known: Vec<&str> = ENUMS
            .iter()
            .map(|e| e.name)
            .chain(STRUCTS.iter().map(|s| s.name))
            .chain(["void", "string", "number"].into_iter())
            .collect();
        let mut refs: Vec<&str> = Vec::new();
        for c in COMMANDS {
            refs.push(c.args_ts);
            refs.push(c.returns_ts);
        }
        for e in EVENTS {
            refs.push(e.payload_ts);
        }
        for r in refs {
            // Strip containers so `Track[]`, `{ clip: Clip }` still resolve.
            let words: Vec<String> = r
                .split(|ch: char| !ch.is_alphanumeric() && ch != '_')
                .filter(|w| {
                    !w.is_empty()
                        && *w != "Record"
                        && *w != "string"
                        && *w != "never"
                        && *w != "number"
                        && *w != "void"
                        && *w != "unknown"
                        && *w != "Promise"
                        && *w != "args"
                        && *w != "clip"
                        && *w != "op"
                        && *w != "value"
                        && *w != "name"
                        && *w != "path"
                        && *w != "target"
                        && *w != "tempo"
                })
                .map(|w| w.to_string())
                .collect();
            for w in words {
                assert!(
                    known.contains(&w.as_str()),
                    "IPC references unknown type `{w}` in `{r}`"
                );
            }
        }
    }
}
