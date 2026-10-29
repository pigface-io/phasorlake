//! Tables and schemas on their way to Python, by the Arrow C data interface
//! (the PyCapsule protocol): pyarrow, polars or nanoarrow read them.
//!
//! The coverage floor of this file is under 100 for its two `#[pymethods]`
//! lines alone: pyo3 makes a wrapper of each method there that llvm-cov
//! counts apart from the call Python makes, and never as run. The methods
//! themselves are run by the tests.

use arrow::ffi::FFI_ArrowSchema;
use arrow::ffi_stream::FFI_ArrowArrayStream;
use arrow_array::{RecordBatch, RecordBatchIterator};
use arrow_schema::{Schema, SchemaRef};
use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use pyo3::types::PyCapsule;

/// A table on its way to Python: one schema, its batches in order.
#[pyclass(frozen, name = "ArrowTable", module = "phasorlake._native")]
pub struct ArrowTable {
    pub schema: SchemaRef,
    pub batches: Vec<RecordBatch>,
}

fn capsule<'py, T: Send + 'static>(
    py: Python<'py>,
    value: T,
    name: &'static std::ffi::CStr,
) -> PyResult<Bound<'py, PyCapsule>> {
    PyCapsule::new_with_value(py, value, name)
}

fn schema_capsule<'py>(py: Python<'py>, schema: &Schema) -> PyResult<Bound<'py, PyCapsule>> {
    let ffi = FFI_ArrowSchema::try_from(schema).map_err(|e| PyException::new_err(e.to_string()))?;
    capsule(py, ffi, c"arrow_schema")
}

#[pymethods]
impl ArrowTable {
    /// The schema, for `pyarrow.schema(...)`.
    fn __arrow_c_schema__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyCapsule>> {
        schema_capsule(py, &self.schema)
    }

    /// The rows as a stream of batches, for `pyarrow.table(...)`.
    #[pyo3(signature = (requested_schema=None))]
    fn __arrow_c_stream__<'py>(
        &self,
        py: Python<'py>,
        requested_schema: Option<&Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, PyCapsule>> {
        // The rows are what the file holds; a reader casts them if it wants to.
        let _ = requested_schema;
        let reader = RecordBatchIterator::new(
            self.batches.clone().into_iter().map(|b| {
                zone!(STREAM);
                Ok(b)
            }),
            self.schema.clone(),
        );
        capsule(
            py,
            FFI_ArrowArrayStream::new(Box::new(reader)),
            c"arrow_array_stream",
        )
    }

    /// Rows.
    fn __len__(&self) -> usize {
        self.batches.iter().map(RecordBatch::num_rows).sum()
    }
}

/// A schema on its way to Python.
#[pyclass(frozen, name = "ArrowSchema", module = "phasorlake._native")]
pub struct ArrowSchema {
    pub schema: SchemaRef,
}

#[pymethods]
impl ArrowSchema {
    /// The schema, for `pyarrow.schema(...)`.
    fn __arrow_c_schema__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyCapsule>> {
        schema_capsule(py, &self.schema)
    }
}
