use std::sync::{Arc, Condvar, Mutex};

use crate::cell::{Cell, CellBuilder, CellContext, CellDataBuilder, CellRefsBuilder};
use crate::error::Error;

// === Delayed Cell Stuff ===

#[derive(Clone)]
pub enum ExtCell {
    Ordinary(Cell),
    Partial(Arc<ExtCellParts>),
    Deferred(Promise<Result<ExtCell, Error>>),
}

impl ExtCell {
    pub fn resolve(mut self, context: &(dyn CellContext + Send + Sync)) -> Result<Cell, Error> {
        loop {
            match self {
                ExtCell::Ordinary(cell) => return Ok(cell),
                ExtCell::Partial(parts) => {
                    let parts = Arc::unwrap_or_clone(parts);

                    let mut refs = CellRefsBuilder::default();
                    for child in parts.refs {
                        let cell = child.resolve(context)?;
                        refs.store_reference(cell)?;
                    }

                    return CellBuilder::from_parts(parts.is_exotic, parts.data.clone(), refs)
                        .build_ext(context);
                }
                ExtCell::Deferred(promise) => {
                    self = ok!(promise.wait_cloned());
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct ExtCellParts {
    pub data: CellDataBuilder,
    pub is_exotic: bool,
    pub refs: Vec<ExtCell>,
}

pub enum ChildrenBuilder {
    Ordinary(CellRefsBuilder),
    Extended(Vec<ExtCell>),
}

impl ChildrenBuilder {
    pub fn store_reference(&mut self, cell: ExtCell) -> Result<(), Error> {
        match (&mut *self, cell) {
            (Self::Ordinary(builder), ExtCell::Ordinary(cell)) => builder.store_reference(cell),
            (Self::Ordinary(builder), cell) => {
                let capacity = builder.len() + 1;
                let Self::Ordinary(builder) =
                    std::mem::replace(self, Self::Extended(Vec::with_capacity(capacity)))
                else {
                    // SAFETY: We have just checked the `self` discriminant.
                    unsafe { std::hint::unreachable_unchecked() }
                };

                let Self::Extended(ext_builder) = self else {
                    // SAFETY: We have just updated the `self` with this value.
                    unsafe { std::hint::unreachable_unchecked() }
                };

                for cell in builder {
                    ext_builder.push(ExtCell::Ordinary(cell.clone()));
                }
                ext_builder.push(cell);
                Ok(())
            }
            (Self::Extended(builder), cell) => {
                builder.push(cell);
                Ok(())
            }
        }
    }
}

// === Promise Stuff ===

#[derive(Clone)]
#[repr(transparent)]
pub struct Promise<T> {
    inner: Arc<(Mutex<Option<T>>, Condvar)>,
}

impl<T> Default for Promise<T> {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Promise<T> {
    pub fn new() -> Self {
        Self {
            inner: Arc::new((Mutex::new(None), Condvar::new())),
        }
    }

    pub fn set(&self, value: T) {
        let (lock, cvar) = &*self.inner;
        let mut data = lock.lock().unwrap();
        *data = Some(value);
        cvar.notify_all();
    }

    pub fn wait_cloned(&self) -> T
    where
        T: Clone,
    {
        let (lock, cvar) = &*self.inner;
        let mut data = lock.lock().unwrap();
        loop {
            match &*data {
                None => data = cvar.wait(data).unwrap(),
                Some(value) => break value.clone(),
            }
        }
    }
}
