//! The apply step Overlay v1.1 and v1.2 share: one `update` / `copy` /
//! `remove` action against the working document, with v1.1's array
//! rule (an array merge value concatenates, anything else is appended).

use crate::apply::{ActionOutcome, ApplyError, ApplyErrorKind, ApplyOptions, Operation};
use crate::common::apply::{compile_path, locate, merge_json, remove_at};
use enumset::EnumSet;
use serde_json::Value;

/// The fields of an action that [`apply_action`] reads, borrowed from
/// a version's own `Action` type.
pub(crate) struct ActionFields<'a> {
    pub target: &'a str,
    pub update: Option<&'a Value>,
    pub copy: Option<&'a str>,
    pub remove: bool,
}

/// How [`apply_action`] treats a `target` node that is a primitive or
/// `null`. Each variant exists only with the version that uses it.
#[derive(Debug, Clone, Copy)]
pub(crate) enum PrimitiveTargets {
    /// Fail with [`ApplyErrorKind::PrimitiveActionTarget`] (Overlay v1.1).
    #[cfg(feature = "v1_1")]
    Reject,
    /// `update` and `copy` replace the node with their (primitive)
    /// value, and `remove` removes it from its container, per
    /// [Overlay v1.2's Action Object](https://spec.openapis.org/overlay/v1.2.0.html#action-object).
    #[cfg(feature = "v1_2")]
    Replace,
}

impl PrimitiveTargets {
    fn are_rejected(self) -> bool {
        match self {
            #[cfg(feature = "v1_1")]
            PrimitiveTargets::Reject => true,
            #[cfg(feature = "v1_2")]
            PrimitiveTargets::Replace => false,
        }
    }
}

/// Apply one action to `doc`. On error `doc` may be partly changed:
/// callers apply to a working copy and commit it only on success.
pub(crate) fn apply_action(
    index: usize,
    action: &ActionFields<'_>,
    doc: &mut Value,
    options: EnumSet<ApplyOptions>,
    primitives: PrimitiveTargets,
) -> Result<ActionOutcome, ApplyError> {
    let err = |kind| ApplyError {
        action_index: index,
        target: action.target.to_owned(),
        kind,
    };
    let outcome = |operation, matched| ActionOutcome {
        index,
        target: action.target.to_owned(),
        operation,
        matched,
    };

    let operation = if action.remove {
        Operation::Remove
    } else if action.copy.is_some() {
        Operation::Copy
    } else {
        Operation::Update
    };

    // No-op fast path (validate flags it; this is the defensive
    // arm for callers that skip validation). Bail before any JSONPath
    // work — a do-nothing action can't affect the document.
    if !action.remove && action.update.is_none() && action.copy.is_none() {
        return Ok(outcome(operation, 0));
    }

    // `update` and `copy` are mutually exclusive (validate flags it).
    // Fail fast rather than silently dropping `update` — there is no
    // spec-defined precedence between the two, unlike `remove`.
    if !action.remove && action.update.is_some() && action.copy.is_some() {
        return Err(err(ApplyErrorKind::ConflictingMergeSources));
    }

    let path =
        compile_path(action.target).map_err(|msg| err(ApplyErrorKind::InvalidJsonPath(msg)))?;
    let pointers = locate(doc, &path);

    if pointers.is_empty() {
        if options.contains(ApplyOptions::ErrorOnZeroMatch) {
            return Err(err(ApplyErrorKind::ZeroMatch));
        }
        return Ok(outcome(operation, 0));
    }

    // Checked up front so a failure leaves the working copy untouched.
    let kinds: Vec<NodeKind> = pointers.iter().map(|p| classify(doc, p)).collect();
    let selects_primitive = kinds.iter().any(|k| matches!(k, NodeKind::Primitive));
    if selects_primitive && primitives.are_rejected() {
        return Err(err(ApplyErrorKind::PrimitiveActionTarget));
    }
    if options.contains(ApplyOptions::ErrorOnMixedKindMatch) && !uniform_kinds(&kinds) {
        return Err(err(ApplyErrorKind::MixedKindMatch));
    }

    if action.remove {
        // Process in reverse to preserve earlier pointer validity
        // when siblings live in the same array. Count only successful
        // removes.
        let mut removed = 0;
        for ptr in pointers.iter().rev() {
            if remove_at(doc, ptr) {
                removed += 1;
            }
        }
        return Ok(outcome(operation, removed));
    }

    // Resolve the effective merge value. For `copy` it must be owned
    // (cloned out of the doc so the later mutation can't alias it);
    // for `update` we borrow the payload to avoid an extra deep clone
    // when every target is an object (`merge_json` only needs `&Value`).
    let copy_value: Option<Value> = if let Some(copy_src) = action.copy {
        let copy_path =
            compile_path(copy_src).map_err(|msg| err(ApplyErrorKind::InvalidJsonPath(msg)))?;
        let src_pointers = locate(doc, &copy_path);
        if src_pointers.is_empty() {
            return Err(err(ApplyErrorKind::CopySourceNotFound(copy_src.to_owned())));
        }
        if src_pointers.len() > 1 {
            return Err(err(ApplyErrorKind::CopySourceMultiple(copy_src.to_owned())));
        }
        Some(
            doc.pointer(&src_pointers[0])
                .expect("located pointer must resolve in same doc snapshot")
                .clone(),
        )
    } else {
        None
    };
    let effective_update: &Value = match &copy_value {
        Some(v) => v,
        None => action
            .update
            .expect("no-op fast path covers the all-None case"),
    };

    // A primitive target is replaced, so its value must be a primitive
    // too (v1.2 Action Object: "the value of this field MUST be a primitive
    // value to replace each selected node").
    if selects_primitive && matches!(effective_update, Value::Object(_) | Value::Array(_)) {
        return Err(err(ApplyErrorKind::NonPrimitiveValueForPrimitiveTarget));
    }

    // Update / copy share the same per-node merge rules. For an array
    // target the merge value either concatenates (when it is itself an
    // array) or is appended as a single entry (object / primitive),
    // per Overlay v1.1 §3.3. Object targets recurse per §4.4.3.1.
    for (ptr, kind) in pointers.iter().zip(kinds.iter()) {
        if let Some(node) = doc.pointer_mut(ptr) {
            match kind {
                NodeKind::Array => {
                    if let Value::Array(arr) = node {
                        match effective_update {
                            Value::Array(items) => arr.extend(items.iter().cloned()),
                            other => arr.push(other.clone()),
                        }
                    }
                }
                NodeKind::Object => merge_json(node, effective_update),
                // Only reached under `PrimitiveTargets::Replace`.
                NodeKind::Primitive => *node = effective_update.clone(),
                // Can't occur: the pointers were just located.
                NodeKind::Missing => {}
            }
        }
    }
    Ok(outcome(operation, pointers.len()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodeKind {
    Object,
    Array,
    Primitive,
    Missing,
}

fn classify(doc: &Value, pointer: &str) -> NodeKind {
    match doc.pointer(pointer) {
        None => NodeKind::Missing,
        Some(Value::Object(_)) => NodeKind::Object,
        Some(Value::Array(_)) => NodeKind::Array,
        Some(_) => NodeKind::Primitive,
    }
}

fn uniform_kinds(kinds: &[NodeKind]) -> bool {
    let mut iter = kinds
        .iter()
        .copied()
        .filter(|k| !matches!(k, NodeKind::Missing));
    let Some(first) = iter.next() else {
        return true;
    };
    iter.all(|k| k == first)
}
