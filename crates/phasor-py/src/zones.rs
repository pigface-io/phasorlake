//! The zones of the native module, for `zonetrace` (feature `zones`): 900 to
//! 999. Zones Python opens through `_zone` take numbers from 1000, one per
//! name, so the Python that calls in and the Rust it calls share one tree.
//!
//! The feature also counts the module's heap: every allocation of its Rust
//! goes through `stats_alloc`, and each of the module's zones keeps what was
//! allocated, reallocated and freed in it (`_memory`). Python's own heap and
//! pyarrow's pool are not the module's, and are not counted here.

use std::cell::RefCell;
use std::sync::Mutex;

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use stats_alloc::{INSTRUMENTED_SYSTEM, Stats, StatsAlloc};
use zonetrace::{Guard, Zone};

#[global_allocator]
static HEAP: &StatsAlloc<std::alloc::System> = &INSTRUMENTED_SYSTEM;

/// What each of the module's zones did to the heap, summed over its calls,
/// in the order the zones were first left.
static SPENT: Mutex<Vec<(&'static str, u64, Stats)>> = Mutex::new(Vec::new());

/// The heap counted from its making to its drop, under the zone's name.
pub struct Heap {
    zone: &'static Zone,
    start: Stats,
}

impl Heap {
    /// Counting starts.
    pub fn enter(zone: &'static Zone) -> Self {
        Self {
            zone,
            start: HEAP.stats(),
        }
    }
}

impl Drop for Heap {
    fn drop(&mut self) {
        let spent = HEAP.stats() - self.start;
        let Ok(mut all) = SPENT.lock() else { return };
        match all.iter_mut().find(|(name, ..)| *name == self.zone.name()) {
            Some((_, calls, sum)) => {
                *calls += 1;
                sum.allocations += spent.allocations;
                sum.deallocations += spent.deallocations;
                sum.reallocations += spent.reallocations;
                sum.bytes_allocated += spent.bytes_allocated;
                sum.bytes_deallocated += spent.bytes_deallocated;
                sum.bytes_reallocated += spent.bytes_reallocated;
            }
            None => all.push((self.zone.name(), 1, spent)),
        }
    }
}

/// The registry and kits built, on the first call.
pub static STACK: Zone = Zone::new(900, "py.stack");
/// One `decode`, from its path to the dict Python gets.
pub static DECODE: Zone = Zone::new(901, "py.decode");
/// The file's entries walked, decoded and conformed into batches.
pub static WALK: Zone = Zone::new(902, "py.walk");
/// What was said of the file raised through Python's `warnings`.
pub static WARN: Zone = Zone::new(903, "py.warn");
/// Each table's batches put under one schema and wrapped for Python.
pub static GATHER: Zone = Zone::new(904, "py.gather");
/// A batch handed across the Arrow C stream to its reader.
pub static STREAM: Zone = Zone::new(905, "py.stream");
/// One `inspect`.
pub static INSPECT: Zone = Zone::new(906, "py.inspect");
/// One `table`.
pub static TABLE: Zone = Zone::new(907, "py.table");

/// Zones named from Python, by name: each leaked once, so it lives as long
/// as the tree that names it.
static NAMED: Mutex<Vec<&'static Zone>> = Mutex::new(Vec::new());

thread_local! {
    /// The guards of the Python zones open on this thread, innermost last.
    static OPEN: RefCell<Vec<Guard>> = const { RefCell::new(Vec::new()) };
}

/// A zone for Python: `with _zone("name"):` times the block into the same
/// tree as the Rust beneath it. Make one per name and keep it; entering is
/// two calls from Python, tens of nanoseconds, so it is for stretches of
/// microseconds and more.
#[pyclass(frozen, name = "_Zone", module = "phasorlake._native")]
pub struct PyZone {
    zone: &'static Zone,
}

#[pymethods]
impl PyZone {
    fn __enter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        let guard = zonetrace::enter(slf.zone);
        OPEN.with_borrow_mut(|o| o.push(guard));
        slf
    }

    #[pyo3(signature = (*_args))]
    fn __exit__(&self, _args: &Bound<'_, pyo3::types::PyTuple>) {
        drop(OPEN.with_borrow_mut(Vec::pop));
    }
}

/// The zone named `name`, made the first time it is asked for.
#[pyfunction]
pub fn _zone(name: &str) -> PyResult<PyZone> {
    let mut named = NAMED
        .lock()
        .map_err(|_| PyRuntimeError::new_err("a zone was being named when a thread panicked"))?;
    if let Some(z) = named.iter().find(|z| z.name() == name) {
        return Ok(PyZone { zone: z });
    }
    let id = u16::try_from(1000 + named.len())
        .map_err(|_| PyRuntimeError::new_err("too many zones named from Python"))?;
    let name: &'static str = Box::leak(name.to_owned().into_boxed_str());
    let zone: &'static Zone = Box::leak(Box::new(Zone::new(id, name)));
    named.push(zone);
    Ok(PyZone { zone })
}

/// What each of the module's zones did to the heap since the last call, one
/// dict a zone: `zone`, `calls`, `allocations`, `reallocations`,
/// `deallocations`, `bytes_allocated` (a reallocation's growth included),
/// `bytes_reallocated` (grown less shrunk, by reallocation), and
/// `bytes_deallocated`; and the module's heap now, `live`.
#[pyfunction]
pub fn _memory(py: Python<'_>) -> PyResult<(Vec<Bound<'_, PyDict>>, i128)> {
    let all = std::mem::take(&mut *SPENT.lock().map_err(|_| {
        PyRuntimeError::new_err("the heap was being counted when a thread panicked")
    })?);
    let mut out = Vec::with_capacity(all.len());
    for (zone, calls, s) in all {
        let d = PyDict::new(py);
        d.set_item("zone", zone)?;
        d.set_item("calls", calls)?;
        d.set_item("allocations", s.allocations)?;
        d.set_item("reallocations", s.reallocations)?;
        d.set_item("deallocations", s.deallocations)?;
        d.set_item("bytes_allocated", s.bytes_allocated)?;
        d.set_item("bytes_reallocated", s.bytes_reallocated)?;
        d.set_item("bytes_deallocated", s.bytes_deallocated)?;
        out.push(d);
    }
    let now = HEAP.stats();
    // A reallocation's growth is in `bytes_allocated` already, its shrinking
    // in `bytes_deallocated`.
    let live = now.bytes_allocated as i128 - now.bytes_deallocated as i128;
    Ok((out, live))
}

/// Every thread's zones since the last call, as zonetrace's table, the
/// tracer's own cost taken out; the trees start again empty.
#[pyfunction]
pub fn _zones() -> String {
    zonetrace::flush();
    zonetrace::take_all().text(zonetrace::overhead())
}
