use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::state::Selection;

const HISTORY_FRAMES: usize = 180;
const RECENT_TABLE_FRAMES: usize = 90;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum PerfSpan {
    Frame = 0,
    AppStartup,
    AppIngress,
    AppPrelude,
    MenuAndTabs,
    Timeline,
    Toolbar,
    Properties,
    AuxiliaryPanels,
    Dialogs,
    ActionDrain,
    Library,
    Stage,
    ToolHandle,
    SceneRender,
    SelectablePlacementHit,
    SelectionOverlay,
    SelectionTransformBounds,
    SelectedBodyHit,
    RawHoverHit,
    SelectionStageMeshBuild,
    SelectionScreenMeshBuild,
    SelectionPaintSplit,
}

impl PerfSpan {
    pub const COUNT: usize = 23;
    pub const ALL: [Self; Self::COUNT] = [
        Self::Frame,
        Self::AppStartup,
        Self::AppIngress,
        Self::AppPrelude,
        Self::MenuAndTabs,
        Self::Timeline,
        Self::Toolbar,
        Self::Properties,
        Self::AuxiliaryPanels,
        Self::Dialogs,
        Self::ActionDrain,
        Self::Library,
        Self::Stage,
        Self::ToolHandle,
        Self::SceneRender,
        Self::SelectablePlacementHit,
        Self::SelectionOverlay,
        Self::SelectionTransformBounds,
        Self::SelectedBodyHit,
        Self::RawHoverHit,
        Self::SelectionStageMeshBuild,
        Self::SelectionScreenMeshBuild,
        Self::SelectionPaintSplit,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Frame => "frame.total",
            Self::AppStartup => "app.startup",
            Self::AppIngress => "app.ingress",
            Self::AppPrelude => "app.prelude",
            Self::MenuAndTabs => "ui.menu+tabs",
            Self::Timeline => "ui.timeline",
            Self::Toolbar => "ui.toolbar",
            Self::Properties => "ui.properties",
            Self::AuxiliaryPanels => "ui.aux-panels",
            Self::Dialogs => "ui.dialogs",
            Self::ActionDrain => "app.action-drain",
            Self::Library => "library.total",
            Self::Stage => "stage.total",
            Self::ToolHandle => "stage.tools",
            Self::SceneRender => "stage.scene",
            Self::SelectablePlacementHit => "selection.placement-hit",
            Self::SelectionOverlay => "selection.overlay",
            Self::SelectionTransformBounds => "selection.transform+bounds",
            Self::SelectedBodyHit => "selection.selected-body-hit",
            Self::RawHoverHit => "selection.raw-hover-hit",
            Self::SelectionStageMeshBuild => "selection.stage-mesh-build",
            Self::SelectionScreenMeshBuild => "selection.screen-mesh-build",
            Self::SelectionPaintSplit => "selection.paint-split",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum PerfCounter {
    SelectionMeshVertices = 0,
    SelectionMeshIndices,
    SelectionPaintCacheMisses,
    SelectionGpuCallbacks,
    SelectionPathCount,
}

impl PerfCounter {
    pub const COUNT: usize = 5;
    pub const ALL: [Self; Self::COUNT] = [
        Self::SelectionMeshVertices,
        Self::SelectionMeshIndices,
        Self::SelectionPaintCacheMisses,
        Self::SelectionGpuCallbacks,
        Self::SelectionPathCount,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::SelectionMeshVertices => "selection.mesh-vertices",
            Self::SelectionMeshIndices => "selection.mesh-indices",
            Self::SelectionPaintCacheMisses => "selection.paint-cache-misses",
            Self::SelectionGpuCallbacks => "selection.gpu-callbacks",
            Self::SelectionPathCount => "selection.path-count",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ScenePerfContext {
    pub zoom: f32,
    pub view_scale: f32,
    pub pan_x: f32,
    pub pan_y: f32,
    pub pointer_over_stage: bool,
    pub cursor_stage: Option<[f32; 2]>,
    pub q0rg_id: u16,
    pub frame: u16,
    pub tool: &'static str,
    pub q0rg_visits: u64,
    pub active_placements: u64,
    pub gpu_callbacks: u64,
    pub filtered_callbacks: u64,
    pub blur_passes: u64,
    pub glow_passes: u64,
    pub filter_pixel_passes: u64,
    pub max_filter_region_pixels: u64,
    pub gpu_timer_initialized: bool,
    pub gpu_timer_supported: bool,
    pub gpu_filter_samples: u64,
    pub gpu_filter_total_ns: u64,
    pub gpu_filter_max_ns: u64,
    pub gpu_filter_dropped_samples: u64,
    pub gpu_filter_cpu_callbacks: u64,
    pub gpu_filter_cpu_total_ns: u64,
    pub gpu_filter_cpu_max_ns: u64,
}

impl Default for ScenePerfContext {
    fn default() -> Self {
        Self {
            zoom: 0.0,
            view_scale: 0.0,
            pan_x: 0.0,
            pan_y: 0.0,
            pointer_over_stage: false,
            cursor_stage: None,
            q0rg_id: 0,
            frame: 0,
            tool: "unknown",
            q0rg_visits: 0,
            active_placements: 0,
            gpu_callbacks: 0,
            filtered_callbacks: 0,
            blur_passes: 0,
            glow_passes: 0,
            filter_pixel_passes: 0,
            max_filter_region_pixels: 0,
            gpu_timer_initialized: false,
            gpu_timer_supported: false,
            gpu_filter_samples: 0,
            gpu_filter_total_ns: 0,
            gpu_filter_max_ns: 0,
            gpu_filter_dropped_samples: 0,
            gpu_filter_cpu_callbacks: 0,
            gpu_filter_cpu_total_ns: 0,
            gpu_filter_cpu_max_ns: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct PerfMetric {
    calls: u32,
    total_ns: u64,
    max_ns: u64,
}

impl PerfMetric {
    fn record(&mut self, duration: Duration) {
        let ns = duration.as_nanos().min(u128::from(u64::MAX)) as u64;
        self.calls = self.calls.saturating_add(1);
        self.total_ns = self.total_ns.saturating_add(ns);
        self.max_ns = self.max_ns.max(ns);
    }

    fn total_ms(self) -> f64 {
        self.total_ns as f64 / 1_000_000.0
    }

    fn max_ms(self) -> f64 {
        self.max_ns as f64 / 1_000_000.0
    }
}

#[derive(Clone, Debug)]
struct PerfFrame {
    index: u64,
    metrics: [PerfMetric; PerfSpan::COUNT],
    counters: [u64; PerfCounter::COUNT],
    scene: Option<ScenePerfContext>,
}

pub struct PerfLog {
    next_frame_index: u64,
    frame_started: Option<Instant>,
    current_metrics: [PerfMetric; PerfSpan::COUNT],
    current_counters: [u64; PerfCounter::COUNT],
    current_scene: Option<ScenePerfContext>,
    history: VecDeque<PerfFrame>,
}

impl Default for PerfLog {
    fn default() -> Self {
        Self {
            next_frame_index: 0,
            frame_started: None,
            current_metrics: [PerfMetric::default(); PerfSpan::COUNT],
            current_counters: [0; PerfCounter::COUNT],
            current_scene: None,
            history: VecDeque::with_capacity(HISTORY_FRAMES),
        }
    }
}

impl PerfLog {
    pub fn begin_frame(&mut self) {
        // If an integration path ever forgets to finish a frame, keep the log
        // coherent instead of silently merging two UI frames together.
        if self.frame_started.is_some() {
            self.finish_frame();
        }
        self.current_metrics = [PerfMetric::default(); PerfSpan::COUNT];
        self.current_counters = [0; PerfCounter::COUNT];
        self.current_scene = None;
        self.frame_started = Some(Instant::now());
    }

    pub fn finish_frame(&mut self) {
        let Some(started) = self.frame_started.take() else {
            return;
        };
        self.record(PerfSpan::Frame, started.elapsed());
        if self.history.len() == HISTORY_FRAMES {
            self.history.pop_front();
        }
        self.history.push_back(PerfFrame {
            index: self.next_frame_index,
            metrics: self.current_metrics,
            counters: self.current_counters,
            scene: self.current_scene,
        });
        self.next_frame_index = self.next_frame_index.wrapping_add(1);
    }

    pub fn record(&mut self, span: PerfSpan, duration: Duration) {
        self.current_metrics[span as usize].record(duration);
    }

    pub fn add_counter(&mut self, counter: PerfCounter, amount: u64) {
        let slot = &mut self.current_counters[counter as usize];
        *slot = slot.saturating_add(amount);
    }

    pub fn set_counter_max(&mut self, counter: PerfCounter, value: u64) {
        let slot = &mut self.current_counters[counter as usize];
        *slot = (*slot).max(value);
    }

    pub fn set_scene_context(&mut self, scene: ScenePerfContext) {
        self.current_scene = Some(scene);
    }

    pub fn snapshot_text(&self, selection: &Selection) -> String {
        let frames: Vec<&PerfFrame> = self.history.iter().collect();
        let mut out = String::with_capacity(32 * 1024);
        out.push_str("q0editor performance snapshot\n");
        out.push_str(&format!("version={}\n", env!("CARGO_PKG_VERSION")));
        out.push_str(&format!(
            "build={}\n",
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }
        ));
        out.push_str(&format!("captured_frames={}\n", frames.len()));
        out.push_str(&format!("selection={}\n", selection_summary(selection)));
        if let Some(scene) = frames.last().and_then(|frame| frame.scene) {
            out.push_str(&format!(
                "latest_scene zoom={:.2}x view_scale={:.3} pan=({:.1},{:.1}) pointer_over_stage={} cursor_stage={:?} q0rg={} frame={} tool={} q0rg_visits={} active_placements={} gpu_callbacks={} filtered_callbacks={} blur_passes={} glow_passes={} filter_work={:.3}Mpp max_region={:.3}Mpx gpu_timer_initialized={} gpu_timer_supported={} gpu_samples={} gpu_avg={:.3}ms gpu_max={:.3}ms gpu_estimate={:.3}ms gpu_cpu_callbacks={} gpu_cpu_avg={:.3}ms gpu_cpu_max={:.3}ms gpu_dropped={}\n",
                scene.zoom,
                scene.view_scale,
                scene.pan_x,
                scene.pan_y,
                scene.pointer_over_stage,
                scene.cursor_stage,
                scene.q0rg_id,
                scene.frame,
                scene.tool,
                scene.q0rg_visits,
                scene.active_placements,
                scene.gpu_callbacks,
                scene.filtered_callbacks,
                scene.blur_passes,
                scene.glow_passes,
                scene.filter_pixel_passes as f64 / 1_000_000.0,
                scene.max_filter_region_pixels as f64 / 1_000_000.0,
                scene.gpu_timer_initialized,
                scene.gpu_timer_supported,
                scene.gpu_filter_samples,
                average_ns_ms(scene.gpu_filter_total_ns, scene.gpu_filter_samples),
                scene.gpu_filter_max_ns as f64 / 1_000_000.0,
                scene.gpu_filter_total_ns as f64
                    * q0glblend::GPU_FILTER_SAMPLE_STRIDE as f64
                    / 1_000_000.0,
                scene.gpu_filter_cpu_callbacks,
                average_ns_ms(
                    scene.gpu_filter_cpu_total_ns,
                    scene.gpu_filter_cpu_callbacks,
                ),
                scene.gpu_filter_cpu_max_ns as f64 / 1_000_000.0,
                scene.gpu_filter_dropped_samples,
            ));
        }
        out.push_str(
            "gpu_work_note=callback counts and filter pixel-work are estimates of workload; sampled GPU times below use non-blocking timer queries when supported\n",
        );
        out.push_str(&format!(
            "gpu_timer_note=GL_TIME_ELAPSED samples 1/{} filtered vector callbacks; results are asynchronous and can trail the rendered scene by one or more frames\n",
            q0glblend::GPU_FILTER_SAMPLE_STRIDE,
        ));
        out.push_str("units=milliseconds unless noted\n\n");

        if frames.is_empty() {
            out.push_str("no completed frames captured yet\n");
            return out;
        }

        out.push_str("summary across captured frames\n");
        for span in PerfSpan::ALL {
            let values: Vec<f64> = frames
                .iter()
                .map(|frame| frame.metrics[span as usize].total_ms())
                .filter(|value| *value > 0.0)
                .collect();
            if values.is_empty() {
                continue;
            }
            let calls: u64 = frames
                .iter()
                .map(|frame| u64::from(frame.metrics[span as usize].calls))
                .sum();
            out.push_str(&format!(
                "{:<31} calls={:<6} avg={:>9.3} p50={:>9.3} p95={:>9.3} p99={:>9.3} max={:>9.3}\n",
                span.label(),
                calls,
                mean(&values),
                percentile(&values, 0.50),
                percentile(&values, 0.95),
                percentile(&values, 0.99),
                values.iter().copied().fold(0.0_f64, f64::max),
            ));
        }

        out.push_str("\nworst frames by total frame time\n");
        let mut worst = frames.clone();
        worst.sort_by(|a, b| {
            b.metrics[PerfSpan::Frame as usize]
                .total_ns
                .cmp(&a.metrics[PerfSpan::Frame as usize].total_ns)
        });
        for frame in worst.into_iter().take(12) {
            append_frame_line(&mut out, frame);
        }

        let scene_frames: Vec<ScenePerfContext> =
            frames.iter().filter_map(|frame| frame.scene).collect();
        if !scene_frames.is_empty() {
            let gpu_samples: u64 = scene_frames
                .iter()
                .map(|scene| scene.gpu_filter_samples)
                .sum();
            let gpu_total_ns: u64 = scene_frames
                .iter()
                .map(|scene| scene.gpu_filter_total_ns)
                .sum();
            let gpu_max_ns = scene_frames
                .iter()
                .map(|scene| scene.gpu_filter_max_ns)
                .max()
                .unwrap_or(0);
            let estimated_per_frame_ms: Vec<f64> = scene_frames
                .iter()
                .filter(|scene| scene.gpu_filter_samples > 0)
                .map(|scene| {
                    scene.gpu_filter_total_ns as f64 * q0glblend::GPU_FILTER_SAMPLE_STRIDE as f64
                        / 1_000_000.0
                })
                .collect();
            let gpu_dropped: u64 = scene_frames
                .iter()
                .map(|scene| scene.gpu_filter_dropped_samples)
                .sum();
            let cpu_callbacks: u64 = scene_frames
                .iter()
                .map(|scene| scene.gpu_filter_cpu_callbacks)
                .sum();
            let cpu_total_ns: u64 = scene_frames
                .iter()
                .map(|scene| scene.gpu_filter_cpu_total_ns)
                .sum();
            let cpu_max_ns = scene_frames
                .iter()
                .map(|scene| scene.gpu_filter_cpu_max_ns)
                .max()
                .unwrap_or(0);
            let timer_status = scene_frames
                .iter()
                .rev()
                .find(|scene| scene.gpu_timer_initialized)
                .map(|scene| {
                    if scene.gpu_timer_supported {
                        "supported"
                    } else {
                        "unsupported"
                    }
                })
                .unwrap_or("not initialized");
            out.push_str("\ngpu filter timing summary\n");
            out.push_str(&format!(
                "timer={} samples={} avg_sample={:.3}ms max_sample={:.3}ms estimated_filter_gpu_ms_per_measured_frame avg={:.3} p95={:.3} max={:.3} measured_frames={} dropped={} cpu_callbacks={} avg_callback_cpu={:.3}ms max_callback_cpu={:.3}ms\n",
                timer_status,
                gpu_samples,
                average_ns_ms(gpu_total_ns, gpu_samples),
                gpu_max_ns as f64 / 1_000_000.0,
                mean(&estimated_per_frame_ms),
                percentile(&estimated_per_frame_ms, 0.95),
                estimated_per_frame_ms.iter().copied().fold(0.0_f64, f64::max),
                estimated_per_frame_ms.len(),
                gpu_dropped,
                cpu_callbacks,
                average_ns_ms(cpu_total_ns, cpu_callbacks),
                cpu_max_ns as f64 / 1_000_000.0,
            ));
        }

        out.push_str("\nrecent frame timeline (oldest -> newest)\n");
        let start = frames.len().saturating_sub(RECENT_TABLE_FRAMES);
        for frame in &frames[start..] {
            append_frame_line(&mut out, frame);
        }

        out.push_str("\npeak counters in captured frames\n");
        for counter in PerfCounter::ALL {
            let peak = frames
                .iter()
                .map(|frame| frame.counters[counter as usize])
                .max()
                .unwrap_or(0);
            if peak > 0 {
                out.push_str(&format!("{:<31} {}\n", counter.label(), peak));
            }
        }
        out
    }

    #[cfg(test)]
    fn history_len(&self) -> usize {
        self.history.len()
    }
}

fn append_frame_line(out: &mut String, frame: &PerfFrame) {
    let m = |span: PerfSpan| frame.metrics[span as usize].total_ms();
    let metric = |span: PerfSpan| frame.metrics[span as usize];
    let c = |counter: PerfCounter| frame.counters[counter as usize];
    let scene = frame.scene.unwrap_or_default();
    out.push_str(&format!(
        "#{:<6} frame={:>9.3} startup={:>8.3} ingress={:>8.3} prelude={:>8.3} menu={:>8.3} timeline={:>8.3} toolbar={:>8.3} properties={:>8.3} aux={:>8.3} dialogs={:>8.3} actions={:>8.3} library={:>8.3} stage={:>8.3} scene={:>8.3} tools={:>8.3} place_hit={:>8.3} zoom={:>5.2}x scale={:>6.3} pan=({:>6.1},{:>6.1}) pointer={} cursor={:?} q0rg={} at={} qvis={} active={} gpu={} fx_gpu={} blur_p={} glow_p={} filter_mpp={:>8.3} max_region_mpx={:>6.3} gpu_samples={} gpu_avg={:>7.3} gpu_max={:>7.3} gpu_est={:>7.3} gpu_cpu={:>7.3} gpu_drop={} sel={:>8.3} transform={:>8.3} body={:>8.3}/{} rawhover={:>8.3}/{} stage_mesh={:>9.3} screen_mesh={:>9.3} mesh_v={} mesh_i={} cache_miss={} gpu_cb={} paths={}\n",
        frame.index,
        m(PerfSpan::Frame),
        m(PerfSpan::AppStartup),
        m(PerfSpan::AppIngress),
        m(PerfSpan::AppPrelude),
        m(PerfSpan::MenuAndTabs),
        m(PerfSpan::Timeline),
        m(PerfSpan::Toolbar),
        m(PerfSpan::Properties),
        m(PerfSpan::AuxiliaryPanels),
        m(PerfSpan::Dialogs),
        m(PerfSpan::ActionDrain),
        m(PerfSpan::Library),
        m(PerfSpan::Stage),
        m(PerfSpan::SceneRender),
        m(PerfSpan::ToolHandle),
        m(PerfSpan::SelectablePlacementHit),
        scene.zoom,
        scene.view_scale,
        scene.pan_x,
        scene.pan_y,
        scene.pointer_over_stage,
        scene.cursor_stage,
        scene.q0rg_id,
        scene.frame,
        scene.q0rg_visits,
        scene.active_placements,
        scene.gpu_callbacks,
        scene.filtered_callbacks,
        scene.blur_passes,
        scene.glow_passes,
        scene.filter_pixel_passes as f64 / 1_000_000.0,
        scene.max_filter_region_pixels as f64 / 1_000_000.0,
        scene.gpu_filter_samples,
        average_ns_ms(scene.gpu_filter_total_ns, scene.gpu_filter_samples),
        scene.gpu_filter_max_ns as f64 / 1_000_000.0,
        scene.gpu_filter_total_ns as f64
            * q0glblend::GPU_FILTER_SAMPLE_STRIDE as f64
            / 1_000_000.0,
        average_ns_ms(
            scene.gpu_filter_cpu_total_ns,
            scene.gpu_filter_cpu_callbacks,
        ),
        scene.gpu_filter_dropped_samples,
        m(PerfSpan::SelectionOverlay),
        m(PerfSpan::SelectionTransformBounds),
        metric(PerfSpan::SelectedBodyHit).max_ms(),
        metric(PerfSpan::SelectedBodyHit).calls,
        metric(PerfSpan::RawHoverHit).max_ms(),
        metric(PerfSpan::RawHoverHit).calls,
        m(PerfSpan::SelectionStageMeshBuild),
        m(PerfSpan::SelectionScreenMeshBuild),
        c(PerfCounter::SelectionMeshVertices),
        c(PerfCounter::SelectionMeshIndices),
        c(PerfCounter::SelectionPaintCacheMisses),
        c(PerfCounter::SelectionGpuCallbacks),
        c(PerfCounter::SelectionPathCount),
    ));
}

fn average_ns_ms(total_ns: u64, count: u64) -> f64 {
    if count == 0 {
        0.0
    } else {
        total_ns as f64 / count as f64 / 1_000_000.0
    }
}

fn selection_summary(selection: &Selection) -> String {
    match selection {
        Selection::None => "none".to_owned(),
        Selection::Asset(id) => format!("asset({id})"),
        Selection::Q0rg(id) => format!("q0rg({id})"),
        Selection::Placement { .. } => "placement".to_owned(),
        Selection::Multi(items) => format!("multi(placements={})", items.len()),
        Selection::Path { .. } => "raw-path(1)".to_owned(),
        Selection::Paths(paths) => format!("raw-paths({})", paths.len()),
        Selection::RawParts {
            fills,
            strokes,
            stroke_segments,
            objects,
        } => format!(
            "raw-parts(fills={}, strokes={}, segments={}, objects={})",
            fills.len(),
            strokes.len(),
            stroke_segments.len(),
            objects.len()
        ),
        Selection::StrokeSegment(_) => "raw-stroke-segment".to_owned(),
        Selection::PathPoints { anchor_indices, .. } => {
            format!("path-points(anchors={})", anchor_indices.len())
        }
        Selection::SubselectPoints { anchor_indices, .. } => {
            format!("subselect-points(anchors={})", anchor_indices.len())
        }
        Selection::RawArea {
            placements,
            objects,
            bounds_min,
            bounds_max,
            clip_polygon,
        } => format!(
            "raw-area(placements={}, objects={}, bounds={:.3}x{:.3}, clip_points={})",
            placements.len(),
            objects.len(),
            (bounds_max.x - bounds_min.x).abs(),
            (bounds_max.y - bounds_min.y).abs(),
            clip_polygon.as_ref().map_or(0, Vec::len),
        ),
        Selection::Mixed { paths, objects } => {
            format!("mixed(paths={}, objects={})", paths.len(), objects.len())
        }
    }
}

fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len().max(1) as f64
}

fn percentile(values: &[f64], quantile: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let index = ((sorted.len() - 1) as f64 * quantile.clamp(0.0, 1.0)).round() as usize;
    sorted[index]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_calls_totals_and_copyable_millisecond_report() {
        let mut log = PerfLog::default();
        log.begin_frame();
        log.record(PerfSpan::SelectedBodyHit, Duration::from_millis(17));
        log.record(PerfSpan::SelectedBodyHit, Duration::from_millis(23));
        log.record(
            PerfSpan::SelectionStageMeshBuild,
            Duration::from_millis(811),
        );
        log.set_counter_max(PerfCounter::SelectionMeshVertices, 769_588);
        log.set_counter_max(PerfCounter::SelectionMeshIndices, 2_307_240);
        log.set_scene_context(ScenePerfContext {
            zoom: 4.0,
            view_scale: 1.25,
            pan_x: 12.0,
            pan_y: -8.0,
            pointer_over_stage: true,
            cursor_stage: Some([400.0, 220.0]),
            q0rg_id: 7,
            frame: 12,
            tool: "Select",
            q0rg_visits: 5,
            active_placements: 143,
            gpu_callbacks: 27,
            filtered_callbacks: 19,
            blur_passes: 48,
            glow_passes: 4,
            filter_pixel_passes: 2_000_000,
            max_filter_region_pixels: 750_000,
            gpu_timer_initialized: true,
            gpu_timer_supported: true,
            gpu_filter_samples: 2,
            gpu_filter_total_ns: 400_000,
            gpu_filter_max_ns: 250_000,
            gpu_filter_dropped_samples: 0,
            gpu_filter_cpu_callbacks: 19,
            gpu_filter_cpu_total_ns: 950_000,
            gpu_filter_cpu_max_ns: 90_000,
        });
        log.finish_frame();

        let text = log.snapshot_text(&Selection::None);
        assert!(text.contains("selection.selected-body-hit"));
        assert!(text.contains("stage_mesh=  811.000"));
        assert!(text.contains("mesh_v=769588"));
        assert!(text.contains("mesh_i=2307240"));
        assert!(text.contains("body=  23.000/2"));
        assert!(text.contains(
            "latest_scene zoom=4.00x view_scale=1.250 pan=(12.0,-8.0) pointer_over_stage=true cursor_stage=Some([400.0, 220.0]) q0rg=7 frame=12 tool=Select"
        ));
        assert!(text.contains("gpu_work_note=callback counts and filter pixel-work are estimates"));
        assert!(text.contains("qvis=5 active=143 gpu=27 fx_gpu=19 blur_p=48 glow_p=4"));
        assert!(text.contains("filter_mpp=   2.000 max_region_mpx= 0.750"));
    }

    #[test]
    fn history_is_bounded() {
        let mut log = PerfLog::default();
        for _ in 0..(HISTORY_FRAMES + 40) {
            log.begin_frame();
            log.finish_frame();
        }
        assert_eq!(log.history_len(), HISTORY_FRAMES);
    }
}
