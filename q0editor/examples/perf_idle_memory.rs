//! Headless measurement of the editor's startup fonts and idle UI frames.
//! Run with `cargo run -p q0editor --example perf_idle_memory --release --locked`.

use egui::{Context, FontDefinitions, FontFamily, FontId};

fn report(label: &str) {
    #[cfg(windows)]
    {
        #[repr(C)]
        #[derive(Default)]
        struct MemoryCounters {
            size: u32,
            page_faults: u32,
            peak_working_set: usize,
            working_set: usize,
            peak_paged_pool: usize,
            paged_pool: usize,
            peak_nonpaged_pool: usize,
            nonpaged_pool: usize,
            pagefile: usize,
            peak_pagefile: usize,
            private_bytes: usize,
        }

        #[link(name = "kernel32")]
        extern "system" {
            fn GetCurrentProcess() -> *mut std::ffi::c_void;
            fn K32GetProcessMemoryInfo(
                process: *mut std::ffi::c_void,
                counters: *mut MemoryCounters,
                size: u32,
            ) -> i32;
        }

        let size = std::mem::size_of::<MemoryCounters>() as u32;
        let mut counters = MemoryCounters {
            size,
            ..Default::default()
        };
        // SAFETY: the pseudo-handle needs no close; the counters buffer matches
        // PROCESS_MEMORY_COUNTERS_EX and is writable for exactly `size` bytes.
        assert_ne!(
            unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, size) },
            0
        );
        println!(
            "{label}: working_set_mib={:.2} private_mib={:.2}",
            counters.working_set as f64 / 1_048_576.0,
            counters.private_bytes as f64 / 1_048_576.0,
        );
    }
    #[cfg(not(windows))]
    println!("{label}: process counters are available on Windows");
}

fn draw(ctx: &Context, frame: usize, multilingual: bool) {
    let output = ctx.run(
        egui::RawInput {
            time: Some(frame as f64 / 60.0),
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 800.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::TopBottomPanel::top("menu").show(ctx, |ui| {
                ui.label("File  Edit  Modify  View  Plugins  Help");
                ui.label("untitled.q1s - q0editor");
            });
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.label("Stage  Library  Properties  Timeline");
                if multilingual {
                    ui.label("Привет · 中文 · 日本語 · 한국어");
                    ui.label(
                        egui::RichText::new("let имя = 42;").family(FontFamily::Name(
                            q0editor::q0lang::fonts::Q0LANG_FAMILY.into(),
                        )),
                    );
                }
            });
        },
    );
    std::hint::black_box(ctx.tessellate(output.shapes, output.pixels_per_point));
}

fn main() {
    report("process_start");
    let ctx = Context::default();
    q0editor::theme::install(&ctx);
    q0editor::q0lang::fonts::install(&ctx, "Consolas");
    draw(&ctx, 0, false);
    report("blank_first_frame");
    let (borrowed, owned) = ctx.fonts(|fonts| {
        fonts.lock().fonts.definitions().font_data.values().fold(
            (0, 0),
            |(borrowed, owned), data| match &data.font {
                std::borrow::Cow::Borrowed(bytes) => (borrowed + bytes.len(), owned),
                std::borrow::Cow::Owned(bytes) => (borrowed, owned + bytes.len()),
            },
        )
    });
    println!("font_source_bytes: borrowed={borrowed} owned={owned}");
    for frame in 1..601 {
        draw(&ctx, frame, false);
    }
    report("blank_after_600_frames");
    draw(&ctx, 601, true);
    ctx.fonts(|fonts| {
        let font = FontId::proportional(13.0);
        for sample in ["Привет", "中文", "日本語", "한국어"] {
            println!("glyphs {sample}: {}", fonts.has_glyphs(&font, sample));
        }
    });
    report("multilingual_frame");
    // Font updates clone the current definitions during inline text editing.
    let definitions = ctx.fonts(|fonts| fonts.lock().fonts.definitions().clone());
    let copy = definitions.clone();
    report("two_font_definition_clones");
    std::hint::black_box((definitions, copy, FontDefinitions::default()));
}
