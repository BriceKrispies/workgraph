//! Compile-time value assertions (section 4.6): every slice, flow state and decision part is a
//! plain value: `Clone + Eq + Debug + Send + Sync + 'static`. A type that gains an `Rc`, a cell, a
//! borrowed field or a non-`Eq` field stops the build here. Later model tasks add their slices
//! and flow states to this list.

use crate::decision::Decision;
use crate::effect::{Effect, EffectResult};
use crate::event::{EventDraft, EventPayload};
use crate::slice::{RegistrySlice, TaskSlice};
use crate::stamp::{IdSeq, Stamp};
use crate::write::{Row, RowKey, Write};

const fn assert_value<T: Clone + Eq + core::fmt::Debug + Send + Sync + 'static>() {}

const _: () = {
    assert_value::<TaskSlice>();
    assert_value::<RegistrySlice>();
    assert_value::<Stamp>();
    assert_value::<IdSeq>();
    assert_value::<Decision<()>>();
    assert_value::<Write>();
    assert_value::<Row>();
    assert_value::<RowKey>();
    assert_value::<EventDraft>();
    assert_value::<EventPayload>();
    assert_value::<Effect>();
    assert_value::<EffectResult>();
};
