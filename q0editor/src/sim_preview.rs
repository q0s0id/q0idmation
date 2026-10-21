//! The live preview behind the physics and particle windows. The simulation
//! runs on a thread of its own, on a copy of the project, once edits have
//! settled; the stage then draws that baked copy. The editor never waits for
//! it, which a long range or a big liquid would otherwise make it do.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use egui::{Color32, Context, Painter};
use q0s_format::v2::ProjectV2;

use crate::app::EditorApp;
use crate::render::StageView;

/// Edits must rest this long before a new preview starts.
const SETTLE: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Physics,
    Particles,
}

#[derive(Debug, Clone, PartialEq)]
struct Request {
    physics: Option<(u16, crate::physics::BakeOptions)>,
    particles: Option<(u16, Option<u16>)>,
}

impl Request {
    fn is_empty(&self) -> bool {
        self.physics.is_none() && self.particles.is_none()
    }
}

struct Built {
    project: ProjectV2,
    physics: Option<crate::physics::Simulation>,
    particles: Option<crate::particles::Simulation>,
}

struct Job {
    request: Request,
    source_revision: u64,
    receiver: mpsc::Receiver<Result<Built, String>>,
    cancel: Arc<AtomicBool>,
}

struct Ready {
    request: Request,
    source_revision: u64,
    project: Arc<ProjectV2>,
    /// Render revision of the copy; never one the real project reaches.
    revision: u64,
    physics: Option<Arc<crate::physics::Simulation>>,
    particles: Option<Arc<crate::particles::Simulation>>,
}

#[derive(Default)]
pub struct SimPreview {
    job: Option<Job>,
    ready: Option<Ready>,
    serial: u64,
    seen_revision: u64,
    changed_at: Option<Instant>,
    failure: Option<(Part, String)>,
}

impl SimPreview {
    fn cancel_job(&mut self) {
        if let Some(job) = self.job.take() {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }
}

/// Keeps the preview in step with the project and the windows. Runs every
/// frame; costs nothing while no preview is on.
pub fn update(app: &mut EditorApp, ctx: &Context) {
    let request = Request {
        physics: crate::physics::preview_request(app),
        particles: crate::particles::preview_request(app),
    };
    let revision = app.state.revision;
    let preview = &mut app.session.sim_preview;
    if request.is_empty() {
        preview.cancel_job();
        preview.ready = None;
        return;
    }
    if preview.seen_revision != revision {
        preview.seen_revision = revision;
        preview.changed_at = Some(Instant::now());
    }

    if let Some(job) = &preview.job {
        match job.receiver.try_recv() {
            Ok(result) => {
                let job = preview.job.take().expect("checked above");
                if job.request == request && job.source_revision == revision {
                    match result {
                        Ok(built) => {
                            preview.serial = preview.serial.wrapping_add(1);
                            preview.failure = None;
                            preview.ready = Some(Ready {
                                request: job.request,
                                source_revision: job.source_revision,
                                project: Arc::new(built.project),
                                revision: (1 << 62) | preview.serial,
                                physics: built.physics.map(Arc::new),
                                particles: built.particles.map(Arc::new),
                            });
                        }
                        Err(error) if error != "cancelled" => {
                            let part = if job.request.physics.is_some() {
                                Part::Physics
                            } else {
                                Part::Particles
                            };
                            preview.failure = Some((part, error));
                            preview.ready = None;
                            app.session.physics.previewing = false;
                            app.session.particles.previewing = false;
                        }
                        Err(_) => {}
                    }
                }
                ctx.request_repaint();
            }
            Err(mpsc::TryRecvError::Empty) => {
                if job.request != request || job.source_revision != revision {
                    preview.cancel_job();
                } else {
                    ctx.request_repaint_after(Duration::from_millis(60));
                    return;
                }
            }
            Err(mpsc::TryRecvError::Disconnected) => preview.job = None,
        }
    }

    let fresh = preview
        .ready
        .as_ref()
        .is_some_and(|ready| ready.request == request && ready.source_revision == revision);
    if fresh || preview.job.is_some() {
        return;
    }
    let waited = preview.changed_at.map_or(SETTLE, |at| at.elapsed());
    if waited < SETTLE || ctx.input(|input| input.pointer.any_down()) {
        ctx.request_repaint_after(SETTLE.saturating_sub(waited).max(Duration::from_millis(30)));
        return;
    }
    let project = app.state.project.clone();
    let (sender, receiver) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let job_request = request.clone();
    let job_cancel = Arc::clone(&cancel);
    let waker = ctx.clone();
    let spawned = std::thread::Builder::new()
        .name("q0 simulation preview".to_string())
        .spawn(move || {
            let result = build(project, &job_request, &job_cancel);
            let _ = sender.send(result);
            waker.request_repaint();
        });
    if spawned.is_ok() {
        app.session.sim_preview.job = Some(Job {
            request,
            source_revision: revision,
            receiver,
            cancel,
        });
        ctx.request_repaint_after(Duration::from_millis(60));
    }
}

fn build(mut project: ProjectV2, request: &Request, cancel: &AtomicBool) -> Result<Built, String> {
    let physics = match request.physics {
        Some((q0rg_id, options)) => {
            let simulation = crate::physics::simulate_with(&project, q0rg_id, Some(cancel))?;
            crate::physics::bake(&mut project, &simulation, options)?;
            Some(simulation)
        }
        None => None,
    };
    let particles = match request.particles {
        Some((q0rg_id, only)) => {
            let simulation =
                crate::particles::simulate_with(&project, q0rg_id, only, Some(cancel))?;
            crate::particles::bake(&mut project, &simulation)?;
            Some(simulation)
        }
        None => None,
    };
    Ok(Built {
        project,
        physics,
        particles,
    })
}

fn fresh(app: &EditorApp) -> Option<&Ready> {
    app.session
        .sim_preview
        .ready
        .as_ref()
        .filter(|ready| ready.source_revision == app.state.revision)
}

/// The baked copy the stage draws instead of the project while a preview is
/// on and up to date.
pub fn stage_project(app: &EditorApp) -> Option<(Arc<ProjectV2>, u64)> {
    fresh(app).map(|ready| (Arc::clone(&ready.project), ready.revision))
}

pub fn physics_simulation(app: &EditorApp) -> Option<Arc<crate::physics::Simulation>> {
    fresh(app).and_then(|ready| ready.physics.clone())
}

pub fn particles_simulation(app: &EditorApp) -> Option<Arc<crate::particles::Simulation>> {
    fresh(app).and_then(|ready| ready.particles.clone())
}

/// A line for the window about the preview of `part`, if it has news.
pub fn status_note(app: &EditorApp, part: Part) -> Option<String> {
    let preview = &app.session.sim_preview;
    if let Some((failed, error)) = &preview.failure {
        if *failed == part {
            return Some(format!("Preview stopped: {error}"));
        }
    }
    let on = match part {
        Part::Physics => app.session.physics.previewing,
        Part::Particles => app.session.particles.previewing,
    };
    if !on {
        return None;
    }
    if preview.job.is_some() || fresh(app).is_none() {
        return Some("Simulating the preview…".to_string());
    }
    Some(
        "Previewing: play or scrub the timeline to watch. Nothing is keyed until you bake."
            .to_string(),
    )
}

/// A label in the stage's top-left corner; `row` stacks several.
pub fn paint_banner(painter: &Painter, view: &StageView, text: &str, row: usize) {
    let galley = painter.layout_no_wrap(
        text.to_string(),
        egui::FontId::proportional(13.0),
        Color32::WHITE,
    );
    let at = view.stage_rect.left_top() + egui::vec2(8.0, 8.0 + row as f32 * 24.0);
    let backdrop = egui::Rect::from_min_size(at, galley.size()).expand2(egui::vec2(6.0, 3.0));
    painter.rect_filled(backdrop, 4.0, Color32::from_black_alpha(170));
    painter.galley(at, galley, Color32::WHITE);
}
