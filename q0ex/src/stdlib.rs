//! Functions every plugin has, with or without the editor: numbers, text,
//! lists, maps, colours and time. Those ending in `!` change what they are
//! given; the others hand back something new.

use std::rc::Rc;

use crate::ast::BinOp;
use crate::interp::{index_value, Host, Interp};
use crate::value::{List, Map, Value};

/// Every standard function, for `has` and for the editor's word list.
pub const NAMES: [&str; 78] = [
    "len",
    "str",
    "num",
    "int",
    "round",
    "floor",
    "ceil",
    "abs",
    "sign",
    "min",
    "max",
    "clamp",
    "lerp",
    "sqrt",
    "sin",
    "cos",
    "tan",
    "asin",
    "acos",
    "atan",
    "atan2",
    "hypot",
    "random",
    "pick",
    "shuffle!",
    "type",
    "upper",
    "lower",
    "trim",
    "split",
    "join",
    "replace",
    "contains",
    "starts",
    "ends",
    "find",
    "slice",
    "chars",
    "push!",
    "pop!",
    "insert!",
    "remove!",
    "clear!",
    "sort!",
    "reverse!",
    "sorted",
    "reversed",
    "map",
    "filter",
    "reduce",
    "sum",
    "first",
    "last",
    "range",
    "any",
    "all",
    "count",
    "unique",
    "flat",
    "zip",
    "keys",
    "values",
    "has",
    "get",
    "delete!",
    "merged",
    "copy",
    "json",
    "parse_json",
    "rgb",
    "rgba",
    "hsv",
    "mix",
    "clock",
    "now",
    "oops!",
    "lines",
    "repeat",
];

pub fn has(name: &str) -> bool {
    NAMES.contains(&name)
}

/// Named numbers: `pi`, `tau`, `infinity`, `nan`.
pub fn constant(name: &str) -> Option<Value> {
    Some(Value::Num(match name {
        "pi" => std::f64::consts::PI,
        "tau" => std::f64::consts::TAU,
        "infinity" => f64::INFINITY,
        "nan" => f64::NAN,
        _ => return None,
    }))
}

const NULL: &Value = &Value::Null;

fn arg(args: &[Value], index: usize) -> &Value {
    args.get(index).unwrap_or(NULL)
}

fn num(args: &[Value], index: usize, name: &str) -> Result<f64, String> {
    match arg(args, index) {
        Value::Num(value) => Ok(*value),
        other => Err(format!(
            "{name} needs a number as its {} value, not {}",
            ordinal(index),
            other.type_name()
        )),
    }
}

fn num_or(args: &[Value], index: usize, name: &str, default: f64) -> Result<f64, String> {
    match arg(args, index) {
        Value::Null => Ok(default),
        _ => num(args, index, name),
    }
}

fn text(args: &[Value], index: usize, name: &str) -> Result<Rc<str>, String> {
    match arg(args, index) {
        Value::Str(text) => Ok(text.clone()),
        other => Err(format!(
            "{name} needs text as its {} value, not {}",
            ordinal(index),
            other.type_name()
        )),
    }
}

fn list(args: &[Value], index: usize, name: &str) -> Result<List, String> {
    match arg(args, index) {
        Value::List(items) => Ok(items.clone()),
        other => Err(format!(
            "{name} needs a list as its {} value, not {}",
            ordinal(index),
            other.type_name()
        )),
    }
}

fn map(args: &[Value], index: usize, name: &str) -> Result<Map, String> {
    match arg(args, index) {
        Value::Map(entries) => Ok(entries.clone()),
        other => Err(format!(
            "{name} needs a map as its {} value, not {}",
            ordinal(index),
            other.type_name()
        )),
    }
}

fn ordinal(index: usize) -> &'static str {
    ["first", "second", "third", "fourth", "fifth"]
        .get(index)
        .copied()
        .unwrap_or("next")
}

/// `list` or the items of a list given as the only argument, as `min` and
/// `max` take them.
fn spread(args: &[Value]) -> Vec<Value> {
    match args {
        [Value::List(items)] => items.borrow().clone(),
        _ => args.to_vec(),
    }
}

/// A small random generator per thread; plugins do not need more.
fn random_unit() -> f64 {
    use std::cell::Cell;
    thread_local! {
        static STATE: Cell<u64> = Cell::new(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0x2545_F491, |since| since.as_nanos() as u64)
                | 1,
        );
    }
    STATE.with(|state| {
        let mut x = state.get();
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        state.set(x);
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    })
}

pub fn range(from: f64, to: f64, step: f64, inclusive: bool) -> Result<Value, String> {
    if step == 0.0 || !step.is_finite() || !from.is_finite() || !to.is_finite() {
        return Err("a range needs finite ends and a step that is not 0".into());
    }
    let span = ((to - from) / step).floor();
    if span > 1_000_000.0 {
        return Err("that range would hold over a million numbers".into());
    }
    let mut items = Vec::new();
    let mut value = from;
    let epsilon = step.abs() * 1e-9;
    loop {
        let done = if step > 0.0 {
            if inclusive {
                value > to + epsilon
            } else {
                value >= to - epsilon
            }
        } else if inclusive {
            value < to - epsilon
        } else {
            value <= to + epsilon
        };
        if done {
            break;
        }
        items.push(Value::Num(value));
        value += step;
    }
    Ok(Value::list(items))
}

pub fn binary(op: BinOp, left: &Value, right: &Value) -> Result<Value, String> {
    use Value::*;
    let nums = || match (left, right) {
        (Num(a), Num(b)) => Some((*a, *b)),
        _ => None,
    };
    let mismatch = || {
        format!(
            "cannot {} a {} and a {}",
            match op {
                BinOp::Add => "add",
                BinOp::Sub => "subtract",
                BinOp::Mul => "multiply",
                BinOp::Div => "divide",
                BinOp::Rem => "take the remainder of",
                BinOp::Pow => "raise",
                _ => "compare",
            },
            left.type_name(),
            right.type_name()
        )
    };
    Ok(match op {
        BinOp::Add => match (left, right) {
            (Num(a), Num(b)) => Num(a + b),
            (Str(_), _) | (_, Str(_)) => Value::str(left.to_text() + &right.to_text()),
            (List(a), List(b)) => {
                let mut items = a.borrow().clone();
                items.extend(b.borrow().iter().cloned());
                Value::list(items)
            }
            _ => return Err(mismatch()),
        },
        BinOp::Mul => match (left, right) {
            (Num(a), Num(b)) => Num(a * b),
            (Str(text), Num(times)) | (Num(times), Str(text)) => {
                let times = times.clamp(0.0, 100_000.0) as usize;
                Value::str(text.repeat(times))
            }
            _ => return Err(mismatch()),
        },
        BinOp::Sub | BinOp::Div | BinOp::Rem | BinOp::Pow => {
            let (a, b) = nums().ok_or_else(mismatch)?;
            Num(match op {
                BinOp::Sub => a - b,
                BinOp::Div => a / b,
                BinOp::Rem => a % b,
                _ => a.powf(b),
            })
        }
        BinOp::Eq => Bool(left.equals(right)),
        BinOp::Ne => Bool(!left.equals(right)),
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            let ordering = match (left, right) {
                (Num(a), Num(b)) => a.partial_cmp(b),
                (Str(a), Str(b)) => Some(a.cmp(b)),
                _ => return Err(mismatch()),
            };
            let Some(ordering) = ordering else {
                return Ok(Bool(false));
            };
            Bool(match op {
                BinOp::Lt => ordering.is_lt(),
                BinOp::Le => ordering.is_le(),
                BinOp::Gt => ordering.is_gt(),
                _ => ordering.is_ge(),
            })
        }
        BinOp::And => {
            if left.truthy() {
                right.clone()
            } else {
                left.clone()
            }
        }
        BinOp::Or => {
            if left.truthy() {
                left.clone()
            } else {
                right.clone()
            }
        }
    })
}

/// Sorting order for `sort!`: numbers, then text, then everything else.
fn compare(a: &Value, b: &Value) -> std::cmp::Ordering {
    match (a, b) {
        (Value::Num(a), Value::Num(b)) => a.total_cmp(b),
        (Value::Str(a), Value::Str(b)) => a.cmp(b),
        (Value::Num(_), _) => std::cmp::Ordering::Less,
        (_, Value::Num(_)) => std::cmp::Ordering::Greater,
        (Value::Str(_), _) => std::cmp::Ordering::Less,
        (_, Value::Str(_)) => std::cmp::Ordering::Greater,
        _ => std::cmp::Ordering::Equal,
    }
}

fn sort_items(
    it: &mut Interp,
    items: &mut [Value],
    by: Option<&Value>,
    host: &mut dyn Host,
) -> Result<(), String> {
    match by {
        None => items.sort_by(compare),
        Some(func) => {
            // Sort by a key the function gives each item.
            let mut keyed = Vec::with_capacity(items.len());
            for item in items.iter() {
                keyed.push((
                    it.call_for_stdlib(func, vec![item.clone()], host)?,
                    item.clone(),
                ));
            }
            keyed.sort_by(|a, b| compare(&a.0, &b.0));
            for (slot, (_, item)) in items.iter_mut().zip(keyed) {
                *slot = item;
            }
        }
    }
    Ok(())
}

fn index_of(len: usize, index: f64) -> Option<usize> {
    let len = len as i64;
    let at = if index < 0.0 {
        len + index as i64
    } else {
        index as i64
    };
    (at >= 0 && at <= len).then_some(at as usize)
}

/// Clamps `a..b` (negative from the end) into `len`.
fn span(len: usize, from: f64, to: Option<f64>) -> (usize, usize) {
    let fix = |index: f64| -> usize {
        let len = len as i64;
        let at = if index < 0.0 {
            len + index as i64
        } else {
            index as i64
        };
        at.clamp(0, len) as usize
    };
    let start = fix(from);
    let end = to.map_or(len, fix);
    (start, end.max(start))
}

pub fn call(
    it: &mut Interp,
    name: &str,
    args: Vec<Value>,
    host: &mut dyn Host,
) -> Result<Value, String> {
    let a = &args;
    Ok(match name {
        "len" => Value::Num(match arg(a, 0) {
            Value::Str(text) => text.chars().count(),
            Value::List(items) => items.borrow().len(),
            Value::Map(entries) => entries.borrow().len(),
            other => {
                return Err(format!(
                    "len needs text, a list or a map, not {}",
                    other.type_name()
                ))
            }
        } as f64),
        "str" => Value::str(arg(a, 0).to_text()),
        "num" => match arg(a, 0) {
            Value::Num(value) => Value::Num(*value),
            Value::Str(text) => text.trim().parse::<f64>().map_or(Value::Null, Value::Num),
            Value::Bool(value) => Value::Num(if *value { 1.0 } else { 0.0 }),
            _ => Value::Null,
        },
        "int" => Value::Num(num(a, 0, name)?.trunc()),
        "round" => {
            let digits = num_or(a, 1, name, 0.0)?.clamp(0.0, 12.0);
            let scale = 10f64.powi(digits as i32);
            Value::Num((num(a, 0, name)? * scale).round() / scale)
        }
        "floor" => Value::Num(num(a, 0, name)?.floor()),
        "ceil" => Value::Num(num(a, 0, name)?.ceil()),
        "abs" => Value::Num(num(a, 0, name)?.abs()),
        "sign" => {
            let value = num(a, 0, name)?;
            Value::Num(if value > 0.0 {
                1.0
            } else if value < 0.0 {
                -1.0
            } else {
                0.0
            })
        }
        "min" | "max" => {
            let items = spread(a);
            let mut best: Option<f64> = None;
            for item in &items {
                let value = item
                    .as_num()
                    .ok_or_else(|| format!("{name} takes numbers, not {}", item.type_name()))?;
                best = Some(match best {
                    None => value,
                    Some(best) if name == "min" => best.min(value),
                    Some(best) => best.max(value),
                });
            }
            best.map_or(Value::Null, Value::Num)
        }
        "clamp" => {
            let (value, low, high) = (num(a, 0, name)?, num(a, 1, name)?, num(a, 2, name)?);
            Value::Num(value.max(low.min(high)).min(high.max(low)))
        }
        "lerp" => {
            let (from, to, t) = (num(a, 0, name)?, num(a, 1, name)?, num(a, 2, name)?);
            Value::Num(from + (to - from) * t)
        }
        "sqrt" => Value::Num(num(a, 0, name)?.sqrt()),
        // Angles are degrees, as everywhere in q0editor.
        "sin" => Value::Num(num(a, 0, name)?.to_radians().sin()),
        "cos" => Value::Num(num(a, 0, name)?.to_radians().cos()),
        "tan" => Value::Num(num(a, 0, name)?.to_radians().tan()),
        "asin" => Value::Num(num(a, 0, name)?.asin().to_degrees()),
        "acos" => Value::Num(num(a, 0, name)?.acos().to_degrees()),
        "atan" => Value::Num(num(a, 0, name)?.atan().to_degrees()),
        "atan2" => Value::Num(num(a, 0, name)?.atan2(num(a, 1, name)?).to_degrees()),
        "hypot" => Value::Num(num(a, 0, name)?.hypot(num(a, 1, name)?)),
        "random" => match (arg(a, 0), arg(a, 1)) {
            (Value::Null, _) => Value::Num(random_unit()),
            (_, Value::Null) => {
                let high = num(a, 0, name)?;
                Value::Num((random_unit() * (high.floor() + 1.0)).floor())
            }
            _ => {
                let (low, high) = (num(a, 0, name)?, num(a, 1, name)?);
                if low.fract() == 0.0 && high.fract() == 0.0 {
                    Value::Num(low + (random_unit() * (high - low + 1.0)).floor())
                } else {
                    Value::Num(low + random_unit() * (high - low))
                }
            }
        },
        "pick" => {
            let items = list(a, 0, name)?;
            let items = items.borrow();
            if items.is_empty() {
                Value::Null
            } else {
                items[(random_unit() * items.len() as f64) as usize % items.len()].clone()
            }
        }
        "shuffle!" => {
            let items = list(a, 0, name)?;
            let mut items = items.borrow_mut();
            for index in (1..items.len()).rev() {
                let other = (random_unit() * (index + 1) as f64) as usize % (index + 1);
                items.swap(index, other);
            }
            Value::Null
        }
        "type" => Value::str(arg(a, 0).type_name()),
        "upper" => Value::str(text(a, 0, name)?.to_uppercase()),
        "lower" => Value::str(text(a, 0, name)?.to_lowercase()),
        "trim" => Value::str(text(a, 0, name)?.trim()),
        "split" => {
            let whole = text(a, 0, name)?;
            let parts: Vec<Value> = match arg(a, 1) {
                Value::Null => whole.split_whitespace().map(Value::str).collect(),
                _ => {
                    let separator = text(a, 1, name)?;
                    if separator.is_empty() {
                        whole.chars().map(|c| Value::str(c.to_string())).collect()
                    } else {
                        whole.split(&*separator).map(Value::str).collect()
                    }
                }
            };
            Value::list(parts)
        }
        "lines" => Value::list(text(a, 0, name)?.lines().map(Value::str).collect()),
        "chars" => Value::list(
            text(a, 0, name)?
                .chars()
                .map(|c| Value::str(c.to_string()))
                .collect(),
        ),
        "join" => {
            let items = list(a, 0, name)?;
            let separator = match arg(a, 1) {
                Value::Null => Rc::from(""),
                _ => text(a, 1, name)?,
            };
            let parts = items
                .borrow()
                .iter()
                .map(Value::to_text)
                .collect::<Vec<_>>();
            Value::str(parts.join(&separator))
        }
        "replace" => {
            let whole = text(a, 0, name)?;
            Value::str(whole.replace(&*text(a, 1, name)?, &text(a, 2, name)?))
        }
        "repeat" => {
            let times = num(a, 1, name)?.clamp(0.0, 100_000.0) as usize;
            Value::str(text(a, 0, name)?.repeat(times))
        }
        "contains" => Value::Bool(match (arg(a, 0), arg(a, 1)) {
            (Value::Str(whole), part) => whole.contains(&*part.to_text()),
            (Value::List(items), item) => items.borrow().iter().any(|other| other.equals(item)),
            (Value::Map(entries), key) => entries.borrow().contains_key(&key.to_text()),
            (other, _) => {
                return Err(format!(
                    "contains looks in text, a list or a map, not {}",
                    other.type_name()
                ))
            }
        }),
        "starts" => Value::Bool(text(a, 0, name)?.starts_with(&*text(a, 1, name)?)),
        "ends" => Value::Bool(text(a, 0, name)?.ends_with(&*text(a, 1, name)?)),
        "find" => Value::Num(match (arg(a, 0), arg(a, 1)) {
            (Value::Str(whole), part) => whole
                .find(&*part.to_text())
                .map_or(-1.0, |at| whole[..at].chars().count() as f64),
            (Value::List(items), Value::Func(_)) => {
                let func = arg(a, 1).clone();
                let items = items.borrow().clone();
                let mut found = -1.0;
                for (index, item) in items.into_iter().enumerate() {
                    if it.call_for_stdlib(&func, vec![item], host)?.truthy() {
                        found = index as f64;
                        break;
                    }
                }
                found
            }
            (Value::List(items), item) => items
                .borrow()
                .iter()
                .position(|other| other.equals(item))
                .map_or(-1.0, |at| at as f64),
            (other, _) => {
                return Err(format!(
                    "find looks in text or a list, not {}",
                    other.type_name()
                ))
            }
        }),
        "slice" => {
            let from = num(a, 1, name)?;
            let to = match arg(a, 2) {
                Value::Null => None,
                _ => Some(num(a, 2, name)?),
            };
            match arg(a, 0) {
                Value::Str(whole) => {
                    let chars = whole.chars().collect::<Vec<_>>();
                    let (start, end) = span(chars.len(), from, to);
                    Value::str(chars[start..end].iter().collect::<String>())
                }
                Value::List(items) => {
                    let items = items.borrow();
                    let (start, end) = span(items.len(), from, to);
                    Value::list(items[start..end].to_vec())
                }
                other => {
                    return Err(format!(
                        "slice cuts text or a list, not {}",
                        other.type_name()
                    ))
                }
            }
        }
        "push!" => {
            let items = list(a, 0, name)?;
            items.borrow_mut().extend(a.iter().skip(1).cloned());
            Value::Null
        }
        "pop!" => list(a, 0, name)?.borrow_mut().pop().unwrap_or_default(),
        "insert!" => {
            let items = list(a, 0, name)?;
            let mut items = items.borrow_mut();
            let at = index_of(items.len(), num(a, 1, name)?)
                .ok_or_else(|| "insert! goes past the end of the list".to_string())?;
            items.insert(at, arg(a, 2).clone());
            Value::Null
        }
        "remove!" => {
            let items = list(a, 0, name)?;
            let mut items = items.borrow_mut();
            match index_of(items.len(), num(a, 1, name)?).filter(|at| *at < items.len()) {
                Some(at) => items.remove(at),
                None => Value::Null,
            }
        }
        "clear!" => {
            match arg(a, 0) {
                Value::List(items) => items.borrow_mut().clear(),
                Value::Map(entries) => entries.borrow_mut().clear(),
                other => {
                    return Err(format!(
                        "clear! empties a list or a map, not {}",
                        other.type_name()
                    ))
                }
            }
            Value::Null
        }
        "sort!" => {
            let items = list(a, 0, name)?;
            let mut sorted = items.borrow().clone();
            sort_items(it, &mut sorted, a.get(1), host)?;
            *items.borrow_mut() = sorted;
            Value::Null
        }
        "reverse!" => {
            list(a, 0, name)?.borrow_mut().reverse();
            Value::Null
        }
        "sorted" => {
            let mut items = list(a, 0, name)?.borrow().clone();
            sort_items(it, &mut items, a.get(1), host)?;
            Value::list(items)
        }
        "reversed" => {
            let mut items = list(a, 0, name)?.borrow().clone();
            items.reverse();
            Value::list(items)
        }
        "map" | "filter" | "any" | "all" | "count" => {
            let items = list(a, 0, name)?.borrow().clone();
            let func = arg(a, 1).clone();
            let mut out = Vec::new();
            let mut hits = 0usize;
            for item in &items {
                let result = if matches!(func, Value::Null) {
                    item.clone()
                } else {
                    it.call_for_stdlib(&func, vec![item.clone()], host)?
                };
                match name {
                    "map" => out.push(result),
                    "filter" if result.truthy() => out.push(item.clone()),
                    _ if result.truthy() => hits += 1,
                    _ => {}
                }
            }
            match name {
                "map" | "filter" => Value::list(out),
                "any" => Value::Bool(hits > 0),
                "all" => Value::Bool(hits == items.len()),
                _ => Value::Num(hits as f64),
            }
        }
        "reduce" => {
            let items = list(a, 0, name)?.borrow().clone();
            let func = arg(a, 1).clone();
            let mut total = arg(a, 2).clone();
            for item in items {
                total = it.call_for_stdlib(&func, vec![total, item], host)?;
            }
            total
        }
        "sum" => {
            let mut total = 0.0;
            for item in list(a, 0, name)?.borrow().iter() {
                total += item
                    .as_num()
                    .ok_or_else(|| format!("sum adds numbers, not {}", item.type_name()))?;
            }
            Value::Num(total)
        }
        "first" => list(a, 0, name)?
            .borrow()
            .first()
            .cloned()
            .unwrap_or_default(),
        "last" => list(a, 0, name)?
            .borrow()
            .last()
            .cloned()
            .unwrap_or_default(),
        "range" => {
            let (from, to) = match arg(a, 1) {
                Value::Null => (0.0, num(a, 0, name)?),
                _ => (num(a, 0, name)?, num(a, 1, name)?),
            };
            let step = num_or(a, 2, name, if from <= to { 1.0 } else { -1.0 })?;
            range(from, to, step, false)?
        }
        "unique" => {
            let mut out: Vec<Value> = Vec::new();
            for item in list(a, 0, name)?.borrow().iter() {
                if !out.iter().any(|other| other.equals(item)) {
                    out.push(item.clone());
                }
            }
            Value::list(out)
        }
        "flat" => {
            let mut out = Vec::new();
            for item in list(a, 0, name)?.borrow().iter() {
                match item {
                    Value::List(inner) => out.extend(inner.borrow().iter().cloned()),
                    other => out.push(other.clone()),
                }
            }
            Value::list(out)
        }
        "zip" => {
            let (left, right) = (list(a, 0, name)?, list(a, 1, name)?);
            let (left, right) = (left.borrow(), right.borrow());
            Value::list(
                left.iter()
                    .zip(right.iter())
                    .map(|(x, y)| Value::list(vec![x.clone(), y.clone()]))
                    .collect(),
            )
        }
        "keys" => Value::list(map(a, 0, name)?.borrow().keys().map(Value::str).collect()),
        "values" => Value::list(map(a, 0, name)?.borrow().values().cloned().collect()),
        "has" => Value::Bool(map(a, 0, name)?.borrow().contains_key(&arg(a, 1).to_text())),
        "get" => {
            let found = index_value(arg(a, 0), arg(a, 1))?;
            if matches!(found, Value::Null) {
                arg(a, 2).clone()
            } else {
                found
            }
        }
        "delete!" => map(a, 0, name)?
            .borrow_mut()
            .remove(&arg(a, 1).to_text())
            .unwrap_or_default(),
        "merged" => {
            let mut out = map(a, 0, name)?.borrow().clone();
            for (key, value) in map(a, 1, name)?.borrow().iter() {
                out.insert(key.clone(), value.clone());
            }
            Value::map(out)
        }
        "copy" => arg(a, 0).deep_copy(),
        "json" => Value::str(arg(a, 0).to_json().to_string()),
        "parse_json" => match serde_json::from_str::<serde_json::Value>(&text(a, 0, name)?) {
            Ok(json) => Value::from_json(&json),
            Err(failure) => return Err(format!("parse_json: {failure}")),
        },
        "rgb" | "rgba" => {
            let channel = |index: usize| -> Result<u8, String> {
                Ok(num(a, index, name)?.round().clamp(0.0, 255.0) as u8)
            };
            let alpha = if name == "rgba" { channel(3)? } else { 255 };
            Value::str(color_text([channel(0)?, channel(1)?, channel(2)?, alpha]))
        }
        "hsv" => {
            let hue = num(a, 0, name)?.rem_euclid(360.0) / 60.0;
            let saturation = num_or(a, 1, name, 1.0)?.clamp(0.0, 1.0);
            let value = num_or(a, 2, name, 1.0)?.clamp(0.0, 1.0);
            let chroma = value * saturation;
            let x = chroma * (1.0 - (hue % 2.0 - 1.0).abs());
            let (r, g, b) = match hue as u32 {
                0 => (chroma, x, 0.0),
                1 => (x, chroma, 0.0),
                2 => (0.0, chroma, x),
                3 => (0.0, x, chroma),
                4 => (x, 0.0, chroma),
                _ => (chroma, 0.0, x),
            };
            let m = value - chroma;
            let byte = |c: f64| ((c + m) * 255.0).round().clamp(0.0, 255.0) as u8;
            Value::str(color_text([byte(r), byte(g), byte(b), 255]))
        }
        "mix" => {
            let from = parse_color(&text(a, 0, name)?).ok_or("mix needs two colours")?;
            let to = parse_color(&text(a, 1, name)?).ok_or("mix needs two colours")?;
            let t = num_or(a, 2, name, 0.5)?.clamp(0.0, 1.0);
            let blend =
                |i: usize| (from[i] as f64 + (to[i] as f64 - from[i] as f64) * t).round() as u8;
            Value::str(color_text([blend(0), blend(1), blend(2), blend(3)]))
        }
        "clock" => Value::Num(it.clock),
        "now" => Value::Num(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0.0, |since| since.as_secs_f64()),
        ),
        "oops!" => {
            let message = a.iter().map(Value::to_text).collect::<Vec<_>>().join(" ");
            return Err(if message.is_empty() {
                "oops!".into()
            } else {
                message
            });
        }
        _ => return Err(format!("there is no function `{name}`")),
    })
}

/// `#rrggbb` or `#rrggbbaa`, as colours are written in q0ex.
pub fn color_text([r, g, b, a]: [u8; 4]) -> String {
    if a == 255 {
        format!("#{r:02x}{g:02x}{b:02x}")
    } else {
        format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
    }
}

/// Reads `#rgb`, `#rrggbb`, `#rrggbbaa` or a few colour names.
pub fn parse_color(text: &str) -> Option<[u8; 4]> {
    let text = text.trim();
    let named = match text.to_ascii_lowercase().as_str() {
        "black" => Some([0, 0, 0, 255]),
        "white" => Some([255, 255, 255, 255]),
        "red" => Some([235, 64, 52, 255]),
        "green" => Some([60, 186, 84, 255]),
        "blue" => Some([52, 120, 246, 255]),
        "yellow" => Some([250, 204, 21, 255]),
        "orange" => Some([249, 128, 22, 255]),
        "purple" => Some([157, 78, 255, 255]),
        "pink" => Some([255, 105, 180, 255]),
        "cyan" => Some([34, 211, 238, 255]),
        "grey" | "gray" => Some([128, 128, 128, 255]),
        "clear" | "transparent" => Some([0, 0, 0, 0]),
        _ => None,
    };
    if named.is_some() {
        return named;
    }
    let hex = text.strip_prefix('#')?;
    let digit = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let bytes = hex.as_bytes();
    match bytes.len() {
        3 | 4 => {
            let mut out = [255u8; 4];
            for (index, c) in bytes.iter().enumerate() {
                let d = digit(*c)?;
                out[index] = d * 17;
            }
            Some(out)
        }
        6 | 8 => {
            let mut out = [255u8; 4];
            for index in 0..bytes.len() / 2 {
                out[index] = digit(bytes[index * 2])? * 16 + digit(bytes[index * 2 + 1])?;
            }
            Some(out)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_round_trip() {
        assert_eq!(parse_color("#ff8000"), Some([255, 128, 0, 255]));
        assert_eq!(parse_color("#f80"), Some([255, 136, 0, 255]));
        assert_eq!(parse_color("#ff800080"), Some([255, 128, 0, 128]));
        assert_eq!(color_text([255, 128, 0, 255]), "#ff8000");
        assert_eq!(parse_color("nope"), None);
    }

    #[test]
    fn ranges_count_both_ways() {
        let numbers = |value: Value| match value {
            Value::List(items) => items
                .borrow()
                .iter()
                .filter_map(Value::as_num)
                .collect::<Vec<_>>(),
            _ => panic!(),
        };
        assert_eq!(
            numbers(range(1.0, 3.0, 1.0, true).unwrap()),
            [1.0, 2.0, 3.0]
        );
        assert_eq!(numbers(range(3.0, 1.0, -1.0, false).unwrap()), [3.0, 2.0]);
        assert_eq!(numbers(range(0.0, 1.0, 0.25, true).unwrap()).len(), 5);
        assert!(range(0.0, 1e9, 1.0, true).is_err());
    }
}
