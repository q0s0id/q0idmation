//! The raster brushes that come with the editor.
//!
//! Each is only a setting of the one engine in `engine.rs`; the user can
//! change any of them and keeps the changes per brush.

use super::engine::{Blend, BrushSettings, Grain, Tip};

/// One brush in the library.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Preset {
    /// Settings key: stays the same when the label changes.
    pub key: &'static str,
    pub label: &'static str,
    pub hint: &'static str,
    pub settings: BrushSettings,
}

/// What a family of tools offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    Paint,
    Erase,
    Smudge,
}

fn base() -> BrushSettings {
    BrushSettings::default()
}

/// Brushes that put paint down.
pub fn paint() -> Vec<Preset> {
    vec![
        Preset {
            key: "pencil",
            label: "Pencil",
            hint: "Graphite on paper: grain shows through a light touch, fills in when you press.",
            settings: BrushSettings {
                size: 3.0,
                hardness: 0.8,
                flow: 0.75,
                spacing: 0.1,
                pressure_size: 0.35,
                pressure_opacity: 0.8,
                min_size: 0.4,
                grain: Grain::Paper,
                grain_amount: 0.65,
                grain_scale: 0.6,
                accumulate: true,
                ..base()
            },
        },
        Preset {
            key: "rough",
            label: "Rough sketch",
            hint: "Loose and light, for construction lines and roughs. Strokes build up as you go over them.",
            settings: BrushSettings {
                size: 5.0,
                hardness: 0.5,
                opacity: 0.55,
                flow: 0.35,
                spacing: 0.12,
                pressure_size: 0.4,
                pressure_opacity: 0.9,
                min_size: 0.3,
                grain: Grain::Paper,
                grain_amount: 0.5,
                accumulate: true,
                ..base()
            },
        },
        Preset {
            key: "ink",
            label: "Ink pen",
            hint: "Clean-up ink: swells with pressure, tapers at both ends, steadied a little.",
            settings: BrushSettings {
                size: 7.0,
                hardness: 0.95,
                spacing: 0.06,
                pressure_size: 0.9,
                min_size: 0.08,
                taper: 18.0,
                stabilizer: 0.25,
                ..base()
            },
        },
        Preset {
            key: "fineliner",
            label: "Fineliner",
            hint: "A technical pen: one even width whatever the pressure.",
            settings: BrushSettings {
                size: 2.5,
                hardness: 1.0,
                spacing: 0.08,
                pressure_size: 0.0,
                stabilizer: 0.15,
                ..base()
            },
        },
        Preset {
            key: "brush_pen",
            label: "Brush pen",
            hint: "A springy brush tip: thick-thin lines with long tapers.",
            settings: BrushSettings {
                size: 14.0,
                hardness: 0.9,
                spacing: 0.05,
                pressure_size: 1.0,
                min_size: 0.05,
                roundness: 0.75,
                follow: true,
                taper: 40.0,
                stabilizer: 0.35,
                ..base()
            },
        },
        Preset {
            key: "calligraphy",
            label: "Calligraphy",
            hint: "A flat nib held at an angle: thick one way, hairline the other.",
            settings: BrushSettings {
                size: 16.0,
                hardness: 0.95,
                spacing: 0.04,
                pressure_size: 0.4,
                min_size: 0.35,
                roundness: 0.14,
                angle: 40.0,
                ..base()
            },
        },
        Preset {
            key: "marker",
            label: "Marker",
            hint: "Even, see-through colour: one stroke never darkens itself, a second one does.",
            settings: BrushSettings {
                size: 18.0,
                hardness: 0.75,
                opacity: 0.6,
                spacing: 0.08,
                pressure_size: 0.15,
                min_size: 0.6,
                roundness: 0.55,
                angle: 30.0,
                ..base()
            },
        },
        Preset {
            key: "round",
            label: "Hard round",
            hint: "Solid colour with a clean edge: flats and fills.",
            settings: BrushSettings {
                size: 24.0,
                hardness: 0.92,
                spacing: 0.08,
                pressure_size: 0.5,
                min_size: 0.3,
                ..base()
            },
        },
        Preset {
            key: "soft",
            label: "Soft round",
            hint: "Soft edged, for shading and glows.",
            settings: BrushSettings {
                size: 60.0,
                hardness: 0.0,
                flow: 0.35,
                opacity: 0.8,
                spacing: 0.08,
                pressure_size: 0.2,
                pressure_opacity: 0.8,
                accumulate: true,
                ..base()
            },
        },
        Preset {
            key: "airbrush",
            label: "Airbrush",
            hint: "Mist that keeps building while you hold still.",
            settings: BrushSettings {
                size: 80.0,
                hardness: 0.0,
                flow: 0.05,
                spacing: 0.1,
                pressure_size: 0.0,
                pressure_opacity: 1.0,
                build_up: true,
                accumulate: true,
                ..base()
            },
        },
        Preset {
            key: "watercolour",
            label: "Watercolour",
            hint: "A wet wash: pools at its edges, lets the paper through.",
            settings: BrushSettings {
                size: 44.0,
                hardness: 0.35,
                opacity: 0.5,
                flow: 0.7,
                spacing: 0.06,
                pressure_size: 0.4,
                pressure_opacity: 0.4,
                min_size: 0.4,
                wet_edges: 0.85,
                grain: Grain::Paper,
                grain_amount: 0.3,
                grain_scale: 1.6,
                scatter: 0.03,
                ..base()
            },
        },
        Preset {
            key: "gouache",
            label: "Gouache",
            hint: "Thick paint on a hairy brush: streaks where it runs dry.",
            settings: BrushSettings {
                size: 32.0,
                hardness: 0.9,
                flow: 0.9,
                spacing: 0.04,
                pressure_size: 0.5,
                pressure_opacity: 0.4,
                min_size: 0.4,
                tip: Tip::Bristles,
                density: 0.8,
                follow: true,
                accumulate: true,
                ..base()
            },
        },
        Preset {
            key: "dry_brush",
            label: "Dry brush",
            hint: "Few hairs, little paint, rough canvas: scratchy texture strokes.",
            settings: BrushSettings {
                size: 42.0,
                hardness: 0.85,
                flow: 0.7,
                spacing: 0.04,
                pressure_size: 0.3,
                pressure_opacity: 0.6,
                min_size: 0.5,
                tip: Tip::Bristles,
                density: 0.25,
                follow: true,
                grain: Grain::Canvas,
                grain_amount: 0.55,
                accumulate: true,
                ..base()
            },
        },
        Preset {
            key: "chalk",
            label: "Chalk",
            hint: "Powdery and broken: the grain stays white in the pits.",
            settings: BrushSettings {
                size: 26.0,
                hardness: 0.6,
                flow: 0.85,
                spacing: 0.1,
                pressure_size: 0.3,
                pressure_opacity: 0.5,
                min_size: 0.5,
                roundness: 0.7,
                size_jitter: 0.15,
                angle_jitter: 0.4,
                grain: Grain::Chalk,
                grain_amount: 0.85,
                accumulate: true,
                ..base()
            },
        },
        Preset {
            key: "charcoal",
            label: "Charcoal",
            hint: "A stick on its side: smudgy, streaked along the paper.",
            settings: BrushSettings {
                size: 20.0,
                hardness: 0.45,
                flow: 0.8,
                spacing: 0.08,
                pressure_size: 0.3,
                pressure_opacity: 0.7,
                min_size: 0.5,
                roundness: 0.35,
                follow: true,
                grain: Grain::Charcoal,
                grain_amount: 0.9,
                accumulate: true,
                ..base()
            },
        },
        Preset {
            key: "crayon",
            label: "Crayon",
            hint: "Waxy: bright colour with paper showing through.",
            settings: BrushSettings {
                size: 10.0,
                hardness: 0.9,
                flow: 0.95,
                spacing: 0.1,
                pressure_size: 0.25,
                pressure_opacity: 0.3,
                min_size: 0.6,
                grain: Grain::Paper,
                grain_amount: 0.75,
                grain_scale: 0.45,
                accumulate: true,
                ..base()
            },
        },
        Preset {
            key: "spray",
            label: "Spray paint",
            hint: "A can: a cloud of drops, denser in the middle.",
            settings: BrushSettings {
                size: 64.0,
                hardness: 1.0,
                flow: 0.9,
                spacing: 0.18,
                pressure_size: 0.3,
                pressure_opacity: 0.6,
                min_size: 0.4,
                tip: Tip::Spray,
                density: 0.5,
                build_up: true,
                accumulate: true,
                ..base()
            },
        },
        Preset {
            key: "speckle",
            label: "Speckle",
            hint: "Scattered dots for texture, stars and dust.",
            settings: BrushSettings {
                size: 8.0,
                hardness: 0.9,
                spacing: 1.4,
                pressure_size: 0.5,
                min_size: 0.3,
                scatter: 2.2,
                size_jitter: 0.8,
                flow_jitter: 0.6,
                accumulate: true,
                ..base()
            },
        },
    ]
}

/// Erasers.
pub fn erase() -> Vec<Preset> {
    let eraser = |settings: BrushSettings| BrushSettings {
        blend: Blend::Erase,
        ..settings
    };
    vec![
        Preset {
            key: "hard_eraser",
            label: "Hard eraser",
            hint: "Takes everything away with a clean edge.",
            settings: eraser(BrushSettings {
                size: 16.0,
                hardness: 0.95,
                spacing: 0.08,
                pressure_size: 0.4,
                min_size: 0.4,
                ..base()
            }),
        },
        Preset {
            key: "soft_eraser",
            label: "Soft eraser",
            hint: "Fades paint out gently.",
            settings: eraser(BrushSettings {
                size: 50.0,
                hardness: 0.0,
                flow: 0.4,
                spacing: 0.08,
                pressure_size: 0.2,
                pressure_opacity: 0.8,
                accumulate: true,
                ..base()
            }),
        },
        Preset {
            key: "pencil_eraser",
            label: "Pencil eraser",
            hint: "A small rubber for tidying lines.",
            settings: eraser(BrushSettings {
                size: 6.0,
                hardness: 0.85,
                spacing: 0.08,
                pressure_size: 0.3,
                min_size: 0.5,
                ..base()
            }),
        },
        Preset {
            key: "kneaded",
            label: "Kneaded eraser",
            hint: "Lifts graphite unevenly, catching the paper's grain.",
            settings: eraser(BrushSettings {
                size: 24.0,
                hardness: 0.5,
                flow: 0.6,
                spacing: 0.1,
                pressure_size: 0.3,
                pressure_opacity: 0.7,
                grain: Grain::Chalk,
                grain_amount: 0.6,
                accumulate: true,
                ..base()
            }),
        },
        Preset {
            key: "big_eraser",
            label: "Big eraser",
            hint: "Clears large areas fast.",
            settings: eraser(BrushSettings {
                size: 160.0,
                hardness: 0.85,
                spacing: 0.1,
                pressure_size: 0.0,
                ..base()
            }),
        },
    ]
}

/// Smudge and blur.
pub fn smudge() -> Vec<Preset> {
    vec![
        Preset {
            key: "finger",
            label: "Finger",
            hint: "Drags the paint under it along the stroke.",
            settings: BrushSettings {
                size: 20.0,
                hardness: 0.4,
                spacing: 0.08,
                pressure_size: 0.4,
                pressure_opacity: 0.6,
                min_size: 0.4,
                blend: Blend::Smudge,
                strength: 0.75,
                ..base()
            },
        },
        Preset {
            key: "blender",
            label: "Blender",
            hint: "Melts neighbouring colours together softly.",
            settings: BrushSettings {
                size: 40.0,
                hardness: 0.0,
                spacing: 0.08,
                pressure_size: 0.2,
                pressure_opacity: 0.6,
                blend: Blend::Smudge,
                strength: 0.45,
                ..base()
            },
        },
        Preset {
            key: "soften",
            label: "Soften",
            hint: "Blurs what is under the brush: soft shadows, out-of-focus edges.",
            settings: BrushSettings {
                size: 36.0,
                hardness: 0.2,
                spacing: 0.1,
                pressure_size: 0.2,
                pressure_opacity: 0.6,
                blend: Blend::Blur,
                strength: 0.8,
                ..base()
            },
        },
    ]
}

pub fn family(family: Family) -> Vec<Preset> {
    match family {
        Family::Paint => paint(),
        Family::Erase => erase(),
        Family::Smudge => smudge(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_unique_and_settings_are_in_range() {
        let all: Vec<Preset> = [Family::Paint, Family::Erase, Family::Smudge]
            .into_iter()
            .flat_map(family)
            .collect();
        let mut keys: Vec<_> = all.iter().map(|preset| preset.key).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), all.len());
        for preset in &all {
            assert_eq!(
                preset.settings.sanitized(),
                preset.settings,
                "{}",
                preset.key
            );
        }
        assert!(paint().len() >= 15);
        assert!(erase().iter().all(|p| p.settings.blend == Blend::Erase));
    }
}
