use std::sync::Arc;

use crate::cell::{Cell, CellBuilder, CellContext, CellDataBuilder, CellRefsBuilder};
use crate::error::Error;
use crate::merkle::promise::Promise;

#[derive(Clone)]
pub enum ExtCell {
    Ordinary(Cell),
    Partial(Arc<CellParts>),
    Deferred(Promise<Result<ExtCell, Error>>),
}

#[derive(Clone)]
pub struct CellParts {
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

pub fn resolve_ext_cell(
    mut cell: ExtCell,
    context: &(dyn CellContext + Send + Sync),
) -> Result<Cell, Error> {
    loop {
        match cell {
            ExtCell::Ordinary(cell) => return Ok(cell),
            ExtCell::Partial(parts) => {
                let parts = Arc::unwrap_or_clone(parts);

                let mut refs = CellRefsBuilder::default();
                for child in parts.refs {
                    let cell = resolve_ext_cell(child, context)?;
                    refs.store_reference(cell)?;
                }

                return CellBuilder::from_parts(parts.is_exotic, parts.data.clone(), refs)
                    .build_ext(context);
            }
            ExtCell::Deferred(promise) => {
                cell = ok!(promise.wait_cloned());
            }
        }
    }
}
