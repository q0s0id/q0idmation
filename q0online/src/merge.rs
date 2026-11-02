//! Three-way merging for concurrent room edits.
//!
//! An edit is prepared against the revision its author last saw. When other
//! people changed the room in between, the edit is merged instead of rejected:
//! a resource changed on one side takes that side; a resource changed on both
//! sides merges field by field; a layer's placements and keyframes and the
//! audio arrangement merge as collections; new assets and layers whose ids are
//! already taken get fresh ids; and document order merges by relative order
//! rather than by absolute index. Two different values for the same field are
//! still a conflict, and the merged document must validate.
use super::*;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// Merge `ours` and `theirs`, two descendants of `base`, into one project.
/// `renumber` gives assets and layers created in `ours` fresh ids when
/// `theirs` already uses those ids for something else. Undo and redo pass
/// `false`: they must restore the original identities or fail.
pub fn merge_projects(
    base: &ProjectV2,
    ours: &ProjectV2,
    theirs: &ProjectV2,
    renumber: bool,
) -> Result<ProjectV2> {
    let renumbered = if renumber {
        renumber_created(base, ours, theirs)
    } else {
        None
    };
    let ours = renumbered.as_ref().unwrap_or(ours);
    let b = entities(base);
    let o = entities(ours);
    let t = entities(theirs);
    let keys: BTreeSet<&ResourceKey> = b.keys().chain(o.keys()).chain(t.keys()).collect();
    let mut merged = Entities::new();
    for key in keys {
        if let Some(value) = merge_resource(theirs, key, b.get(key), o.get(key), t.get(key))? {
            merged.insert(key.clone(), value);
        }
    }
    merge_order(&mut merged, &b, &o, &t)?;
    let project = project_from_entities(&merged).map_err(|e| unmergeable(e.message))?;
    validate_project(&project).map_err(|e| match e.code {
        ErrorCode::TooLarge => e,
        _ => unmergeable(e.message),
    })?;
    Ok(project)
}

fn unmergeable(detail: String) -> OnlineError {
    error(
        ErrorCode::Conflict,
        format!("these edits can't be combined: {detail}"),
    )
}
fn conflict(project: &ProjectV2, key: &ResourceKey) -> OnlineError {
    error(
        ErrorCode::Conflict,
        format!(
            "someone else changed {} at the same time",
            resource_label(project, key)
        ),
    )
}

fn position_mut(value: &mut ResourceValue) -> Option<&mut usize> {
    match value {
        ResourceValue::Q0rg(q) => Some(&mut q.position),
        ResourceValue::Layer(l) => Some(&mut l.position),
        ResourceValue::Asset(a) => Some(&mut a.position),
        ResourceValue::LibraryFolder { position } => Some(position),
        ResourceValue::Meta(_) | ResourceValue::Runtime(_) | ResourceValue::AudioClips(_) => None,
    }
}
fn position(value: &ResourceValue) -> usize {
    match value {
        ResourceValue::Q0rg(q) => q.position,
        ResourceValue::Layer(l) => l.position,
        ResourceValue::Asset(a) => a.position,
        ResourceValue::LibraryFolder { position } => *position,
        ResourceValue::Meta(_) | ResourceValue::Runtime(_) | ResourceValue::AudioClips(_) => 0,
    }
}
/// Equal apart from the document index, without copying either value.
fn same_content(a: Option<&ResourceValue>, b: Option<&ResourceValue>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(ResourceValue::Layer(a)), Some(ResourceValue::Layer(b))) => {
            a.layer == b.layer && a.metadata == b.metadata
        }
        (Some(ResourceValue::Asset(a)), Some(ResourceValue::Asset(b))) => {
            a.asset == b.asset
                && a.name == b.name
                && a.appearance == b.appearance
                && a.library_folder == b.library_folder
        }
        (Some(ResourceValue::Q0rg(a)), Some(ResourceValue::Q0rg(b))) => {
            a.q0rg_id == b.q0rg_id
                && a.name == b.name
                && a.frame_count == b.frame_count
                && a.script == b.script
                && a.library_folder == b.library_folder
        }
        (Some(ResourceValue::LibraryFolder { .. }), Some(ResourceValue::LibraryFolder { .. })) => {
            true
        }
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}
/// Content without its document index; order is merged separately.
fn unpositioned(value: Option<&ResourceValue>) -> Option<ResourceValue> {
    value.map(|value| {
        let mut value = value.clone();
        if let Some(position) = position_mut(&mut value) {
            *position = 0;
        }
        value
    })
}

fn merge_resource(
    theirs: &ProjectV2,
    key: &ResourceKey,
    b: Option<&ResourceValue>,
    o: Option<&ResourceValue>,
    t: Option<&ResourceValue>,
) -> Result<Option<ResourceValue>> {
    if same_content(o, b) {
        return Ok(t.cloned());
    }
    if same_content(t, b) || same_content(t, o) {
        return Ok(o.cloned());
    }
    let (Some(sb), Some(so), Some(st)) = (unpositioned(b), unpositioned(o), unpositioned(t)) else {
        // created on both sides, or changed on one side and deleted on the other
        return Err(conflict(theirs, key));
    };
    let json = |value: &ResourceValue| serde_json::to_value(value).ok();
    let (Some(jb), Some(jo), Some(jt)) = (json(&sb), json(&so), json(&st)) else {
        return Err(conflict(theirs, key));
    };
    merge_json(&mut Vec::new(), &jb, &jo, &jt)
        .and_then(|value| serde_json::from_value::<ResourceValue>(value).ok())
        .map(Some)
        .ok_or_else(|| conflict(theirs, key))
}

/// Arrays merged as collections instead of single values. Paths are relative
/// to a serialized `ResourceValue` (`{"kind": ..., "value": ...}`).
const COLLECTIONS: &[&[&str]] = &[
    &["value", "layer", "placements"],
    &["value", "layer", "explicit_keyframes"],
    &["value"],
];

fn merge_json(path: &mut Vec<String>, b: &Value, o: &Value, t: &Value) -> Option<Value> {
    if o == b {
        return Some(t.clone());
    }
    if t == b || t == o {
        return Some(o.clone());
    }
    match (b, o, t) {
        (Value::Object(bm), Value::Object(om), Value::Object(tm)) => {
            let null = Value::Null;
            let keys: BTreeSet<&String> = bm.keys().chain(om.keys()).chain(tm.keys()).collect();
            let mut out = serde_json::Map::new();
            for key in keys {
                path.push(key.clone());
                let merged = merge_json(
                    path,
                    bm.get(key).unwrap_or(&null),
                    om.get(key).unwrap_or(&null),
                    tm.get(key).unwrap_or(&null),
                );
                path.pop();
                if om.contains_key(key) || tm.contains_key(key) {
                    out.insert(key.clone(), merged?);
                }
            }
            Some(Value::Object(out))
        }
        (Value::Array(ba), Value::Array(oa), Value::Array(ta))
            if COLLECTIONS.iter().any(|candidate| {
                candidate
                    .iter()
                    .copied()
                    .eq(path.iter().map(String::as_str))
            }) =>
        {
            let mut merged = merge_list(ba, oa, ta)?;
            if path.last().is_some_and(|last| last == "explicit_keyframes") {
                merged.sort_by_key(|frame| frame.as_u64());
                merged.dedup();
            }
            Some(Value::Array(merged))
        }
        _ => None,
    }
}

/// Multiset three-way merge: apply what `ours` removed and added relative to
/// `base` onto `theirs`. Additions land after their nearest earlier neighbour
/// in `ours`. Removing something `theirs` already changed or removed fails.
fn merge_list(base: &[Value], ours: &[Value], theirs: &[Value]) -> Option<Vec<Value>> {
    let key = |value: &Value| value.to_string();
    let mut removed: HashMap<String, usize> = HashMap::new();
    for value in base {
        *removed.entry(key(value)).or_default() += 1;
    }
    let mut added = Vec::new();
    for (index, value) in ours.iter().enumerate() {
        match removed.get_mut(&key(value)) {
            Some(count) if *count > 0 => *count -= 1,
            _ => added.push(index),
        }
    }
    let mut result = Vec::with_capacity(theirs.len() + added.len());
    for value in theirs {
        if let Some(count) = removed.get_mut(&key(value)) {
            if *count > 0 {
                *count -= 1;
                continue;
            }
        }
        result.push(value.clone());
    }
    if removed.values().any(|count| *count > 0) {
        return None;
    }
    for index in added {
        let anchor = ours[..index]
            .iter()
            .rev()
            .find_map(|previous| result.iter().rposition(|value| value == previous));
        result.insert(anchor.map_or(0, |at| at + 1), ours[index].clone());
    }
    Some(result)
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Collection {
    Q0rgs,
    Layers(u16),
    Assets,
    Folders,
}
fn collection(key: &ResourceKey) -> Option<Collection> {
    match key {
        ResourceKey::Q0rg { .. } => Some(Collection::Q0rgs),
        ResourceKey::Layer { q0rg_id, .. } => Some(Collection::Layers(*q0rg_id)),
        ResourceKey::Asset { .. } => Some(Collection::Assets),
        ResourceKey::LibraryFolder { .. } => Some(Collection::Folders),
        _ => None,
    }
}
fn ordered(entities: &Entities) -> BTreeMap<Collection, Vec<ResourceKey>> {
    let mut groups = BTreeMap::<Collection, Vec<(usize, ResourceKey)>>::new();
    for (key, value) in entities {
        if let Some(group) = collection(key) {
            groups
                .entry(group)
                .or_default()
                .push((position(value), key.clone()));
        }
    }
    groups
        .into_iter()
        .map(|(group, mut items)| {
            items.sort();
            (group, items.into_iter().map(|(_, key)| key).collect())
        })
        .collect()
}

/// Give every positioned resource a canonical index from merged relative order.
fn merge_order(merged: &mut Entities, b: &Entities, o: &Entities, t: &Entities) -> Result<()> {
    let (bo, oo, to) = (ordered(b), ordered(o), ordered(t));
    let present = ordered(merged);
    let empty = Vec::new();
    for (group, keys) in present {
        let order = merge_sequence(
            bo.get(&group).unwrap_or(&empty),
            oo.get(&group).unwrap_or(&empty),
            to.get(&group).unwrap_or(&empty),
        )
        .ok_or_else(|| {
            error(
                ErrorCode::Conflict,
                "someone else reordered the same items at the same time",
            )
        })?;
        let wanted: BTreeSet<&ResourceKey> = keys.iter().collect();
        let mut final_order: Vec<ResourceKey> = order
            .into_iter()
            .filter(|key| wanted.contains(key))
            .collect();
        let placed: BTreeSet<ResourceKey> = final_order.iter().cloned().collect();
        final_order.extend(keys.into_iter().filter(|key| !placed.contains(key)));
        for (index, key) in final_order.iter().enumerate() {
            if let Some(slot) = merged.get_mut(key).and_then(position_mut) {
                *slot = index;
            }
        }
    }
    Ok(())
}

/// Merge two reorderings of `base`. The side that reordered existing items
/// leads; the other side's insertions and deletions are applied to it.
fn merge_sequence(
    base: &[ResourceKey],
    ours: &[ResourceKey],
    theirs: &[ResourceKey],
) -> Option<Vec<ResourceKey>> {
    if ours == base {
        return Some(theirs.to_vec());
    }
    if theirs == base || theirs == ours {
        return Some(ours.to_vec());
    }
    let set = |list: &[ResourceKey]| list.iter().cloned().collect::<BTreeSet<_>>();
    let (bs, os, ts) = (set(base), set(ours), set(theirs));
    let relative = |list: &[ResourceKey], keep: &BTreeSet<ResourceKey>| {
        list.iter()
            .filter(|key| keep.contains(*key))
            .cloned()
            .collect::<Vec<_>>()
    };
    let common_o: BTreeSet<_> = bs.intersection(&os).cloned().collect();
    let common_t: BTreeSet<_> = bs.intersection(&ts).cloned().collect();
    let ours_moved = relative(base, &common_o) != relative(ours, &common_o);
    let theirs_moved = relative(base, &common_t) != relative(theirs, &common_t);
    let (primary, secondary, secondary_set) = if ours_moved && theirs_moved {
        let all: BTreeSet<_> = common_o.intersection(&ts).cloned().collect();
        if relative(ours, &all) != relative(theirs, &all) {
            return None;
        }
        (ours, theirs, &ts)
    } else if ours_moved {
        (ours, theirs, &ts)
    } else {
        (theirs, ours, &os)
    };
    let mut result: Vec<ResourceKey> = primary
        .iter()
        .filter(|key| !bs.contains(*key) || secondary_set.contains(*key))
        .cloned()
        .collect();
    let mut placed: BTreeSet<ResourceKey> = result.iter().cloned().collect();
    for (index, key) in secondary.iter().enumerate() {
        if bs.contains(key) || placed.contains(key) {
            continue;
        }
        let anchor = secondary[..index]
            .iter()
            .rev()
            .find_map(|previous| result.iter().position(|item| item == previous));
        result.insert(anchor.map_or(0, |at| at + 1), key.clone());
        placed.insert(key.clone());
    }
    Some(result)
}

// --- renumbering new ids that someone else already took ------------------------

fn set_asset_id(asset: &mut Asset, id: u16) {
    match asset {
        Asset::Bitmap(a) => a.asset_id = id,
        Asset::Vector(a) => a.asset_id = id,
        Asset::Q0v(a) => a.asset_id = id,
        Asset::Rig(a) => a.asset_id = id,
        Asset::Model(a) => a.asset_id = id,
    }
}
fn fresh(used: &mut HashSet<u16>) -> Option<u16> {
    let next = used
        .iter()
        .max()
        .copied()
        .unwrap_or(0)
        .checked_add(1)
        .filter(|id| *id > 0)
        .or_else(|| (1..=u16::MAX).find(|id| !used.contains(id)))?;
    used.insert(next);
    Some(next)
}
fn remap_asset(project: &mut ProjectV2, old: u16, new: u16) {
    for asset in &mut project.assets {
        if asset.id() == old {
            set_asset_id(asset, new);
        }
    }
    if let Some(name) = project.asset_names.remove(&old) {
        project.asset_names.insert(new, name);
    }
    if let Some(appearance) = project.asset_appearances.remove(&old) {
        project.asset_appearances.insert(new, appearance);
    }
    let folders = &mut project.runtime.library_item_folders;
    if let Some(path) = folders.remove(&LibraryItemReference::Asset(old)) {
        folders.insert(LibraryItemReference::Asset(new), path);
    }
    for placement in project
        .q0rgs
        .iter_mut()
        .flat_map(|q| &mut q.layers)
        .flat_map(|layer| &mut layer.placements)
    {
        if placement.target == v2::Target::Asset(old) {
            placement.target = v2::Target::Asset(new);
        }
    }
    for clip in &mut project.audio_clips {
        if clip.asset_id == old {
            clip.asset_id = new;
        }
    }
}
fn remap_layer(project: &mut ProjectV2, q0rg_id: u16, old: u16, new: u16) {
    if let Some(q0rg) = project.q0rgs.iter_mut().find(|q| q.q0rg_id == q0rg_id) {
        for layer in &mut q0rg.layers {
            if layer.layer_id == old {
                layer.layer_id = new;
            }
        }
    }
    let metadata = std::mem::take(&mut project.layer_metadata);
    project.layer_metadata = metadata
        .into_iter()
        .map(|(key, mut meta)| {
            if key.q0rg_id != q0rg_id {
                return (key, meta);
            }
            if meta.parent_folder_id == Some(old) {
                meta.parent_folder_id = Some(new);
            }
            if meta.mask_layer_id == Some(old) {
                meta.mask_layer_id = Some(new);
            }
            let key = if key.layer_id == old {
                LayerKey::new(q0rg_id, new)
            } else {
                key
            };
            (key, meta)
        })
        .collect();
    for clip in &mut project.audio_clips {
        if clip.q0rg_id == q0rg_id && clip.layer_id == old {
            clip.layer_id = new;
        }
    }
    for script in &mut project.runtime.frame_scripts {
        if script.q0rg_id == q0rg_id && script.layer_id == old {
            script.layer_id = new;
        }
    }
}
/// A copy of `ours` whose newly created assets and layers no longer collide
/// with different resources `theirs` created under the same ids.
fn renumber_created(base: &ProjectV2, ours: &ProjectV2, theirs: &ProjectV2) -> Option<ProjectV2> {
    let ids = |p: &ProjectV2| p.assets.iter().map(Asset::id).collect::<HashSet<_>>();
    let base_assets = ids(base);
    let asset = |p: &ProjectV2, id: u16| p.assets.iter().find(|a| a.id() == id).cloned();
    let mut asset_clashes: Vec<u16> = ours
        .assets
        .iter()
        .map(Asset::id)
        .filter(|id| !base_assets.contains(id))
        .filter(|id| asset(theirs, *id).is_some_and(|other| Some(other) != asset(ours, *id)))
        .collect();
    asset_clashes.sort_unstable();
    let layer_ids = |p: &ProjectV2, q: u16| {
        p.q0rgs
            .iter()
            .find(|x| x.q0rg_id == q)
            .map(|x| x.layers.iter().map(|l| l.layer_id).collect::<HashSet<_>>())
            .unwrap_or_default()
    };
    let layer = |p: &ProjectV2, q: u16, l: u16| {
        p.q0rgs
            .iter()
            .find(|x| x.q0rg_id == q)
            .and_then(|x| x.layers.iter().find(|y| y.layer_id == l))
            .cloned()
    };
    let mut layer_clashes = Vec::new();
    for q0rg in &ours.q0rgs {
        let base_layers = layer_ids(base, q0rg.q0rg_id);
        for l in &q0rg.layers {
            if !base_layers.contains(&l.layer_id)
                && layer(theirs, q0rg.q0rg_id, l.layer_id).is_some_and(|other| other != *l)
            {
                layer_clashes.push((q0rg.q0rg_id, l.layer_id));
            }
        }
    }
    if asset_clashes.is_empty() && layer_clashes.is_empty() {
        return None;
    }
    let mut project = ours.clone();
    let mut used: HashSet<u16> = ids(ours).union(&ids(theirs)).copied().collect();
    for old in asset_clashes {
        let new = fresh(&mut used)?;
        remap_asset(&mut project, old, new);
    }
    for (q0rg_id, old) in layer_clashes {
        let mut used: HashSet<u16> = layer_ids(&project, q0rg_id)
            .union(&layer_ids(theirs, q0rg_id))
            .copied()
            .collect();
        let new = fresh(&mut used)?;
        remap_layer(&mut project, q0rg_id, old, new);
    }
    Some(project)
}
