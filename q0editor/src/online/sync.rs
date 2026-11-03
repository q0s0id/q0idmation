//! CPU-heavy validation and reconciliation stay off the egui thread.
use super::*;
use std::sync::mpsc;

pub(super) struct Job {
    pub local_revision: u64,
    pub room_revision: u64,
    pub rx: mpsc::Receiver<Result<Outcome, String>>,
}

pub(super) struct Outcome {
    pub confirmed: ProjectV2,
    pub project: ProjectV2,
    pub revision: u64,
    pub pending: Option<Pending>,
    pub patch: Option<ProjectPatch>,
    pub rejected: Option<ProjectV2>,
    pub acknowledged: bool,
}

/// Fold room events into the confirmed document and carry this editor's
/// unshared work across them. Unshared work is merged, not replayed: other
/// people's strokes on the same layer, new ids they took and order changes
/// combine with yours. Only a genuine same-field clash sets work aside.
pub(super) fn calculate(
    mut confirmed: ProjectV2,
    mut project: ProjectV2,
    mut revision: u64,
    mut pending: Option<Pending>,
    events: Vec<RoomEvent>,
    prepare_patch: bool,
) -> Result<Outcome, String> {
    let mut rejected = None;
    let mut acknowledged = false;
    for event in events {
        if event.revision != revision + 1 {
            return Err("room event sequence has a gap".into());
        }
        let own = pending.as_ref().is_some_and(|p| p.id == event.operation_id);
        let previous = confirmed.clone();
        // the server validated this event before accepting it
        event
            .patch
            .apply_trusted(&mut confirmed)
            .map_err(|e| e.to_string())?;
        revision = event.revision;
        // what this editor's document was built on before the event arrived
        let base = if own {
            acknowledged = true;
            pending.take().map(|p| p.sent_project).unwrap_or(previous)
        } else {
            if let Some(waiting) = &mut pending {
                // keep the in-flight edit's view current, so its acknowledgement
                // is compared against what the server will actually return
                waiting.sent_project = q0online::merge_projects_trusted(
                    &previous,
                    &waiting.sent_project,
                    &confirmed,
                    true,
                )
                .unwrap_or_else(|_| confirmed.clone());
            }
            previous
        };
        project = if project == base {
            confirmed.clone()
        } else {
            match q0online::merge_projects_trusted(&base, &project, &confirmed, true) {
                Ok(merged) => merged,
                Err(_) => {
                    rejected = Some(project);
                    confirmed.clone()
                }
            }
        };
    }
    let patch = if prepare_patch && pending.is_none() {
        Some(if confirmed == project {
            ProjectPatch {
                changes: Vec::new(),
            }
        } else {
            ProjectPatch::diff(&confirmed, &project).map_err(|e| e.to_string())?
        })
    } else {
        None
    };
    Ok(Outcome {
        confirmed,
        project,
        revision,
        pending,
        patch,
        rejected,
        acknowledged,
    })
}

impl Job {
    pub fn start(
        room: &RoomSession,
        project: ProjectV2,
        local_revision: u64,
        prepare_patch: bool,
        ctx: &Context,
    ) -> Self {
        let confirmed = room.confirmed.clone();
        let pending = room.pending.clone();
        let revision = room.revision;
        let mut next = revision + 1;
        let mut events = Vec::new();
        while let Some(event) = room.events.get(&next) {
            events.push(event.clone());
            next += 1;
        }
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = calculate(confirmed, project, revision, pending, events, prepare_patch);
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        Self {
            local_revision,
            room_revision: revision,
            rx,
        }
    }
}
