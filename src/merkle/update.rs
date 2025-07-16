#[cfg(all(feature = "rayon", feature = "sync"))]
use std::sync::Arc;

#[cfg(all(feature = "rayon", feature = "sync"))]
use super::ext_cell::{CellParts as ExtCellParts, ChildrenBuilder, ExtCell, resolve_ext_cell};
#[cfg(all(feature = "rayon", feature = "sync"))]
use super::promise::Promise;
use super::{FilterAction, MerkleFilter, MerkleProofBuilder, make_pruned_branch};
use crate::cell::*;
use crate::error::Error;
use crate::util::unlikely;

/// Parsed Merkle update representation.
///
/// NOTE: Serialized into `MerkleUpdate` cell.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MerkleUpdate {
    /// Representation hash of the original cell.
    pub old_hash: HashBytes,
    /// Representation hash of the updated cell.
    pub new_hash: HashBytes,
    /// Representation depth of the original cell.
    pub old_depth: u16,
    /// Representation depth of the updated cell.
    pub new_depth: u16,
    /// Partially pruned tree with unchanged cells of the origin cell.
    #[cfg_attr(feature = "serde", serde(with = "crate::boc::Boc"))]
    pub old: Cell,
    /// Partially pruned tree with all cells that are not in the original cell.
    #[cfg_attr(feature = "serde", serde(with = "crate::boc::Boc"))]
    pub new: Cell,
}

impl Eq for MerkleUpdate {}
impl PartialEq for MerkleUpdate {
    fn eq(&self, other: &Self) -> bool {
        self.old_hash == other.old_hash
            && self.new_hash == other.new_hash
            && self.old_depth == other.old_depth
            && self.new_depth == other.new_depth
            && self.old.as_ref() == other.old.as_ref()
            && self.new.as_ref() == other.new.as_ref()
    }
}

impl Default for MerkleUpdate {
    fn default() -> Self {
        let empty_cell = Cell::empty_cell();
        Self {
            old_hash: *EMPTY_CELL_HASH,
            new_hash: *EMPTY_CELL_HASH,
            old_depth: 0,
            new_depth: 0,
            old: empty_cell.clone(),
            new: empty_cell,
        }
    }
}

impl<'a> LoadCell<'a> for MerkleUpdate {
    fn load_from_cell(cell: &'a DynCell) -> Result<Self, Error> {
        if unlikely(!cell.is_exotic()) {
            return Err(Error::UnexpectedOrdinaryCell);
        }

        let mut s = cell.as_slice_allow_exotic();
        if s.size_bits() != Self::BITS || s.size_refs() != Self::REFS {
            return Err(Error::CellUnderflow);
        }

        if ok!(s.load_u8()) != CellType::MerkleUpdate.to_byte() {
            return Err(Error::InvalidCell);
        }

        let res = Self {
            old_hash: ok!(s.load_u256()),
            new_hash: ok!(s.load_u256()),
            old_depth: ok!(s.load_u16()),
            new_depth: ok!(s.load_u16()),
            old: ok!(s.load_reference_cloned()),
            new: ok!(s.load_reference_cloned()),
        };
        debug_assert!(s.is_data_empty() && s.is_refs_empty());

        if res.old.as_ref().hash(0) == &res.old_hash
            && res.old.as_ref().depth(0) == res.old_depth
            && res.new.as_ref().hash(0) == &res.new_hash
            && res.new.as_ref().depth(0) == res.new_depth
        {
            Ok(res)
        } else {
            Err(Error::InvalidCell)
        }
    }
}

impl Store for MerkleUpdate {
    fn store_into(&self, b: &mut CellBuilder, _: &dyn CellContext) -> Result<(), Error> {
        if !b.has_capacity(Self::BITS, Self::REFS) {
            return Err(Error::CellOverflow);
        }

        b.set_exotic(true);
        ok!(b.store_u8(CellType::MerkleUpdate.to_byte()));
        ok!(b.store_u256(&self.old_hash));
        ok!(b.store_u256(&self.new_hash));
        ok!(b.store_u32(((self.old_depth as u32) << 16) | self.new_depth as u32));
        ok!(b.store_reference(self.old.clone()));
        b.store_reference(self.new.clone())
    }
}

#[cfg(feature = "arbitrary")]
impl<'a> arbitrary::Arbitrary<'a> for MerkleUpdate {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        let old = Cell::arbitrary(u).and_then(crate::arbitrary::check_max_depth)?;
        let new = Cell::arbitrary(u).and_then(crate::arbitrary::check_max_depth)?;
        Ok(Self {
            old_hash: *old.hash(0),
            new_hash: *new.hash(0),
            old_depth: old.depth(0),
            new_depth: new.depth(0),
            old,
            new,
        })
    }

    fn size_hint(_: usize) -> (usize, Option<usize>) {
        (4, None)
    }
}

impl MerkleUpdate {
    /// The number of data bits that the Merkle update occupies.
    pub const BITS: u16 = 8 + (256 + 16) * 2;
    /// The number of references that the Merkle update occupies.
    pub const REFS: u8 = 2;

    /// Starts building a Merkle update between the specified cells,
    /// using old cells determined by filter.
    pub fn create<'a, F>(old: &'a DynCell, new: &'a DynCell, f: F) -> MerkleUpdateBuilder<'a, F>
    where
        F: MerkleFilter + 'a,
    {
        MerkleUpdateBuilder::new(old, new, f)
    }

    /// Tries to apply this Merkle update to the specified cell,
    /// producing a new cell and using an empty cell context.
    pub fn apply(&self, old: &Cell) -> Result<Cell, Error> {
        self.apply_ext(old, Cell::empty_context())
    }

    /// Tries to apply this Merkle update to the specified cell,
    /// producing a new cell and using an empty cell context.
    pub fn apply_ext(&self, old: &Cell, context: &dyn CellContext) -> Result<Cell, Error> {
        if old.as_ref().repr_hash() != &self.old_hash {
            return Err(Error::InvalidData);
        }

        if self.old_hash == self.new_hash {
            return Ok(old.clone());
        }

        struct Applier<'a> {
            old_cells: ahash::HashMap<HashBytes, Cell>,
            new_cells: ahash::HashMap<HashBytes, Cell>,
            context: &'a dyn CellContext,
        }

        impl Applier<'_> {
            fn run(&mut self, cell: &DynCell, merkle_depth: u8) -> Result<Cell, Error> {
                let descriptor = cell.descriptor();
                let child_merkle_depth = merkle_depth + descriptor.cell_type().is_merkle() as u8;

                // Start building a new cell
                let mut result = CellBuilder::new();
                result.set_exotic(descriptor.is_exotic());

                // Build all child cells
                let mut children_mask = LevelMask::EMPTY;
                for child in cell.references().cloned() {
                    let child_descriptor = child.as_ref().descriptor();

                    let child = if child_descriptor.is_pruned_branch() {
                        // Replace pruned branches with old cells
                        let mask = child_descriptor.level_mask();
                        if mask.to_byte() & (1 << child_merkle_depth) != 0 {
                            // Use original hash for pruned branches
                            let child_hash = child.as_ref().hash(mask.level() - 1);
                            match self.old_cells.get(child_hash) {
                                Some(cell) => cell.clone(),
                                None => return Err(Error::InvalidData),
                            }
                        } else {
                            child
                        }
                    } else {
                        // Build a child cell if it hasn't been built before
                        let child_hash = child.as_ref().hash(child_merkle_depth);
                        if let Some(child) = self.new_cells.get(child_hash) {
                            child.clone()
                        } else {
                            let child = ok!(self.run(child.as_ref(), child_merkle_depth));
                            self.new_cells.insert(*child_hash, child.clone());
                            child
                        }
                    };

                    children_mask |= child.as_ref().level_mask();
                    _ = result.store_reference(child);
                }

                _ = result.store_cell_data(cell);

                result.build_ext(self.context)
            }
        }

        // Collect old cells
        let old_cells = {
            // Collect and check old cells tree
            let old_cell_hashes = self.find_old_cells()?;

            let mut visited = ahash::HashSet::default();
            let mut old_cells = ahash::HashMap::default();

            // Insert root
            let mut merkle_depth = 0u8;

            visited.insert(old.repr_hash());
            old_cells.insert(*old.hash(merkle_depth), old.clone());
            merkle_depth += old.descriptor().is_merkle() as u8;
            let mut stack = vec![old.references()];

            'outer: while let Some(iter) = stack.last_mut() {
                let cloned = iter.clone().cloned();
                for (child_ref, child) in std::iter::zip(&mut *iter, cloned) {
                    if !visited.insert(child_ref.repr_hash()) {
                        continue;
                    }

                    let hash = child_ref.hash(merkle_depth);
                    if !old_cell_hashes.contains(hash) {
                        // Skip new cells
                        continue;
                    }

                    // Store an owned cell with original merkle depth
                    old_cells.insert(*hash, child);

                    // Increase the current merkle depth if needed
                    merkle_depth += child_ref.descriptor().is_merkle() as u8;
                    // And proceed to processing this child
                    stack.push(child_ref.references());
                    continue 'outer;
                }

                // Decrease the current merkle depth if needed
                merkle_depth -= iter.cell().descriptor().is_merkle() as u8;
                // And return to the previous depth
                stack.pop();
            }

            old_cells
        };

        // Apply changed cells
        let new = Applier {
            old_cells,
            new_cells: Default::default(),
            context,
        }
        .run(self.new.as_ref(), 0)?;

        if new.as_ref().repr_hash() == &self.new_hash {
            Ok(new)
        } else {
            Err(Error::InvalidData)
        }
    }

    /// Tries to apply Merkle update in parallel
    #[cfg(all(feature = "rayon", feature = "sync"))]
    pub fn par_apply(&self, old: &Cell) -> Result<Cell, Error> {
        self.par_apply_ext(old, Cell::empty_context())
    }

    /// Tries to apply Merkle update in parallel
    #[cfg(all(feature = "rayon", feature = "sync"))]
    pub fn par_apply_ext(
        &self,
        old: &Cell,
        context: &(dyn CellContext + Send + Sync),
    ) -> Result<Cell, Error> {
        const SPLIT_DEPTH: u16 = 5;

        if old.as_ref().repr_hash() != &self.old_hash {
            return Err(Error::InvalidData);
        }

        if self.old_hash == self.new_hash {
            return Ok(old.clone());
        }

        struct Applier<'a> {
            old_cells: dashmap::DashMap<HashBytes, Cell, ahash::RandomState>,
            new_cells: dashmap::DashMap<HashBytes, Cell, ahash::RandomState>,
            context: &'a (dyn CellContext + Send + Sync),
        }

        impl Applier<'_> {
            fn run<'scope>(
                &'scope self,
                cell: &DynCell,
                merkle_depth: u8,
                traverse_depth: u16,
                scope: Option<&rayon::Scope<'scope>>,
            ) -> Result<ExtCell, Error> {
                let mut children = ChildrenBuilder::Ordinary(Default::default());

                let descriptor = cell.descriptor();
                let child_merkle_depth = merkle_depth + descriptor.cell_type().is_merkle() as u8;

                for child in cell.references().cloned() {
                    let child_descriptor = child.as_ref().descriptor();

                    let child = if child_descriptor.is_pruned_branch() {
                        // Replace pruned branches with old cells
                        let mask = child_descriptor.level_mask();
                        if mask.to_byte() & (1 << child_merkle_depth) != 0 {
                            // Use original hash for pruned branches
                            let child_hash = child.as_ref().hash(mask.level() - 1);
                            match self.old_cells.get(child_hash) {
                                Some(cell) => ExtCell::Ordinary(cell.clone()),
                                None => return Err(Error::InvalidData),
                            }
                        } else {
                            ExtCell::Ordinary(child)
                        }
                    } else {
                        // Build a child cell if it hasn't been built before
                        let child_hash = child.as_ref().hash(child_merkle_depth);
                        if let Some(child) = self.new_cells.get(child_hash) {
                            ExtCell::Ordinary(child.clone())
                        } else {
                            let child = match scope {
                                Some(scope)
                                    if traverse_depth > SPLIT_DEPTH
                                        && child.repr_depth() > SPLIT_DEPTH =>
                                {
                                    let promise = Promise::new();
                                    let merkle_depth = child_merkle_depth;
                                    let traverse_depth = traverse_depth + 1;
                                    scope.spawn({
                                        let promise = promise.clone();
                                        let child = child.clone();
                                        move |_| {
                                            let cell = self.run(
                                                child.as_ref(),
                                                merkle_depth,
                                                traverse_depth,
                                                None,
                                            );
                                            promise.set(cell);
                                        }
                                    });
                                    ExtCell::Deferred(promise)
                                }
                                _ => {
                                    ok!(self.run(
                                        child.as_ref(),
                                        child_merkle_depth,
                                        traverse_depth + 1,
                                        scope
                                    ))
                                }
                            };

                            if let ExtCell::Ordinary(cell) = &child {
                                self.new_cells.insert(*child_hash, cell.clone());
                            }

                            child
                        }
                    };

                    children.store_reference(child)?;
                }

                // Build the cell
                let cell = match children {
                    ChildrenBuilder::Ordinary(children) => {
                        let mut builder = CellBuilder::new();
                        builder.set_exotic(cell.is_exotic());
                        _ = builder.store_cell_data(cell);
                        builder.set_references(children);
                        let new_cell = ok!(builder.build_ext(self.context));

                        ExtCell::Ordinary(new_cell)
                    }
                    ChildrenBuilder::Extended(refs) => {
                        let mut builder = CellDataBuilder::new();
                        builder.store_cell_data(cell)?;
                        ExtCell::Partial(Arc::new(ExtCellParts {
                            data: builder,
                            is_exotic: cell.is_exotic(),
                            refs,
                        }))
                    }
                };

                Ok(cell)
            }
        }

        // Collect old cells
        let old_cells = {
            // Collect and check old cells tree
            let old_cell_hashes = self.par_find_old_cells()?;

            let visited = Default::default();
            let old_cells = Default::default();

            rayon::scope(|scope| {
                fn traverse<'a>(
                    (cell_ref, cell): (&'a DynCell, Cell),
                    merkle_depth: u8,
                    traverse_depth: u16,
                    scope: Option<&rayon::Scope<'a>>,
                    visited: &'a dashmap::DashSet<HashBytes, ahash::RandomState>,
                    old_cells: &'a dashmap::DashMap<HashBytes, Cell, ahash::RandomState>,
                    old_cell_hashes: &'a dashmap::DashSet<HashBytes, ahash::RandomState>,
                ) {
                    if !visited.insert(*cell_ref.repr_hash()) {
                        return;
                    }

                    let hash = cell_ref.hash(merkle_depth);
                    if !old_cell_hashes.contains(hash) {
                        // Skip new cells
                        return;
                    }

                    // Store an owned cell with original merkle depth
                    old_cells.insert(*hash, cell);

                    let next_merkle_depth = merkle_depth + cell_ref.descriptor().is_merkle() as u8;
                    let next_traverse_depth = traverse_depth + 1;

                    let mut iter = cell_ref.references();
                    let cloned = iter.clone().cloned();
                    for (child_ref, child) in std::iter::zip(&mut iter, cloned) {
                        match scope {
                            Some(scope)
                                if traverse_depth > SPLIT_DEPTH
                                    && child.repr_depth() > SPLIT_DEPTH =>
                            {
                                scope.spawn(move |_| {
                                    traverse(
                                        (child_ref, child),
                                        next_merkle_depth,
                                        next_traverse_depth,
                                        None,
                                        visited,
                                        old_cells,
                                        old_cell_hashes,
                                    );
                                });
                            }
                            _ => {
                                traverse(
                                    (child_ref, child),
                                    next_merkle_depth,
                                    next_traverse_depth,
                                    scope,
                                    visited,
                                    old_cells,
                                    old_cell_hashes,
                                );
                            }
                        }
                    }

                    debug_assert_eq!(
                        merkle_depth,
                        next_merkle_depth - iter.cell().descriptor().is_merkle() as u8,
                    );
                }

                traverse(
                    (old.as_ref(), old.clone()),
                    0,
                    0,
                    Some(scope),
                    &visited,
                    &old_cells,
                    &old_cell_hashes,
                );
            });

            old_cells.into_iter().collect()
        };

        // Apply changed cells
        let new = {
            let applier = Applier {
                old_cells,
                new_cells: Default::default(),
                context,
            };

            let new = rayon::scope(|scope| applier.run(self.new.as_ref(), 0, 0, Some(scope)))?;
            resolve_ext_cell(new, context)?
        };

        if new.as_ref().repr_hash() == &self.new_hash {
            Ok(new)
        } else {
            Err(Error::InvalidData)
        }
    }

    /// Computes the removed cells diff using the original cell.
    pub fn compute_removed_cells<'a>(
        &self,
        old: &'a DynCell,
    ) -> Result<ahash::HashMap<&'a HashBytes, u32>, Error> {
        use std::collections::hash_map;

        if old.repr_hash() != &self.old_hash || self.old.hash(0) != old.repr_hash() {
            return Err(Error::InvalidData);
        }

        if self.old_hash == self.new_hash {
            // No cells were removed
            return Ok(Default::default());
        }

        let mut new_cells = ahash::HashSet::default();

        // Compute a list of all hashes in the `new` merkle update tree
        {
            // TODO: check if `new_cells` set can be used instead of `visited`
            let mut visited = ahash::HashSet::default();
            let mut merkle_depth = self.new.descriptor().is_merkle() as u8;
            let mut stack = vec![self.new.references()];

            visited.insert(self.new.repr_hash());
            new_cells.insert(self.new.hash(0));

            'outer: while let Some(iter) = stack.last_mut() {
                for child in &mut *iter {
                    if !visited.insert(child.repr_hash()) {
                        continue;
                    }

                    // Track new cells
                    new_cells.insert(child.hash(merkle_depth));

                    // Unchanged cells (as pruned branches) must be presented in the old tree
                    let descriptor = child.descriptor();
                    if descriptor.is_pruned_branch() {
                        continue;
                    }

                    // Increase the current merkle depth if needed
                    merkle_depth += descriptor.is_merkle() as u8;
                    // And proceed to processing this child
                    stack.push(child.references());
                    continue 'outer;
                }

                merkle_depth -= iter.cell().descriptor().is_merkle() as u8;
                stack.pop();
            }

            debug_assert_eq!(merkle_depth, 0);
        }

        // Traverse old cells
        let mut result = ahash::HashMap::default();
        result.insert(old.repr_hash(), 1);

        let mut stack = Vec::new();
        if !new_cells.contains(old.repr_hash()) {
            stack.push(old.references());
        }

        'outer: while let Some(iter) = stack.last_mut() {
            for child in &mut *iter {
                let hash = child.repr_hash();
                match result.entry(hash) {
                    hash_map::Entry::Occupied(mut entry) => {
                        *entry.get_mut() += 1;
                        continue;
                    }
                    hash_map::Entry::Vacant(entry) => {
                        entry.insert(1);
                    }
                }

                // Skip empty or used subtrees
                if child.reference_count() == 0 || new_cells.contains(hash) {
                    continue;
                }

                stack.push(child.references());
                continue 'outer;
            }
            stack.pop();
        }

        Ok(result)
    }

    fn find_old_cells(&self) -> Result<ahash::HashSet<&HashBytes>, Error> {
        let mut visited = ahash::HashSet::default();
        let mut old_cells = ahash::HashSet::default();

        // Traverse old cells
        let mut merkle_depth = 0u8;

        // Insert root
        visited.insert(self.old.repr_hash());
        old_cells.insert(self.old.hash(merkle_depth));
        merkle_depth += self.old.descriptor().is_merkle() as u8;
        let mut stack = vec![self.old.references()];

        'outer: while let Some(iter) = stack.last_mut() {
            for child in &mut *iter {
                if !visited.insert(child.repr_hash()) {
                    continue;
                }

                // Store cell with original merkle depth
                old_cells.insert(child.hash(merkle_depth));

                // Skip children for pruned branches
                let descriptor = child.descriptor();
                if descriptor.is_pruned_branch() {
                    continue;
                }

                // Increase the current merkle depth if needed
                merkle_depth += descriptor.is_merkle() as u8;
                // And proceed to processing this child
                stack.push(child.references());
                continue 'outer;
            }

            // Decrease the current merkle depth if needed
            merkle_depth -= iter.cell().descriptor().is_merkle() as u8;
            // And return to the previous depth
            stack.pop();
        }

        debug_assert_eq!(merkle_depth, 0);

        // Traverse new cells

        // Insert root
        visited.clear();
        visited.insert(self.new.repr_hash());
        stack.push(self.new.references());
        merkle_depth += self.new.descriptor().is_merkle() as u8;

        'outer: while let Some(iter) = stack.last_mut() {
            for child in &mut *iter {
                // Skip visited cells
                if !visited.insert(child.repr_hash()) {
                    continue;
                }

                // Unchanged cells (as pruned branches) must be presented in the old tree
                let descriptor = child.descriptor();
                if descriptor.is_pruned_branch() {
                    if descriptor.level_mask().level() == merkle_depth + 1
                        && !old_cells.contains(child.hash(merkle_depth))
                    {
                        return Err(Error::InvalidData);
                    }
                } else {
                    // Increase the current merkle depth if needed
                    merkle_depth += descriptor.is_merkle() as u8;
                    // And proceed to processing this child
                    stack.push(child.references());
                    continue 'outer;
                }
            }

            // Decrease the current merkle depth if needed
            merkle_depth -= iter.cell().descriptor().is_merkle() as u8;
            // And return to the previous depth
            stack.pop();
        }

        debug_assert_eq!(merkle_depth, 0);

        // Done
        Ok(old_cells)
    }

    #[cfg(all(feature = "rayon", feature = "sync"))]
    fn par_find_old_cells(&self) -> Result<dashmap::DashSet<HashBytes, ahash::RandomState>, Error> {
        const SPLIT_DEPTH: u16 = 5;

        let visited = Default::default();
        let old_cells = Default::default();

        rayon::scope(|scope| {
            fn traverse_old_cells<'a>(
                cell: &'a DynCell,
                merkle_depth: u8,
                traverse_depth: u16,
                scope: Option<&rayon::Scope<'a>>,
                visited: &'a dashmap::DashSet<HashBytes, ahash::RandomState>,
                result: &'a dashmap::DashSet<HashBytes, ahash::RandomState>,
            ) {
                if !visited.insert(*cell.repr_hash()) {
                    return;
                }

                result.insert(*cell.hash(merkle_depth));

                let descriptor = cell.descriptor();

                if descriptor.is_pruned_branch() {
                    return;
                }

                let next_merkle_depth = merkle_depth + descriptor.is_merkle() as u8;
                let next_traverse_depth = traverse_depth + 1;

                let mut iter = cell.references();
                for child in &mut iter {
                    match scope {
                        Some(scope)
                            if traverse_depth > SPLIT_DEPTH && child.repr_depth() > SPLIT_DEPTH =>
                        {
                            scope.spawn(move |_| {
                                traverse_old_cells(
                                    child,
                                    next_merkle_depth,
                                    next_traverse_depth,
                                    None,
                                    visited,
                                    result,
                                );
                            });
                        }
                        _ => {
                            traverse_old_cells(
                                child,
                                next_merkle_depth,
                                next_traverse_depth,
                                scope,
                                visited,
                                result,
                            );
                        }
                    }
                }

                debug_assert_eq!(
                    merkle_depth,
                    next_merkle_depth - iter.cell().descriptor().is_merkle() as u8,
                );
            }

            traverse_old_cells(self.old.as_ref(), 0, 0, Some(scope), &visited, &old_cells);
        });

        visited.clear();

        rayon::scope(|scope| {
            fn traverse_new_cells<'a>(
                cell: &'a DynCell,
                merkle_depth: u8,
                traverse_depth: u16,
                scope: Option<&rayon::Scope<'a>>,
                visited: &'a dashmap::DashSet<HashBytes, ahash::RandomState>,
                old_cells: &'a dashmap::DashSet<HashBytes, ahash::RandomState>,
            ) {
                if !visited.insert(*cell.repr_hash()) {
                    return;
                }

                let descriptor = cell.descriptor();

                if descriptor.is_pruned_branch() {
                    let level_ok = descriptor.level_mask().level() == merkle_depth + 1;
                    debug_assert!(!level_ok || old_cells.contains(cell.hash(merkle_depth)),);
                }

                let next_merkle_depth = merkle_depth + descriptor.is_merkle() as u8;
                let next_traverse_depth = traverse_depth + 1;

                let mut iter = cell.references();
                for child in &mut iter {
                    match scope {
                        Some(scope)
                            if traverse_depth > SPLIT_DEPTH && child.repr_depth() > SPLIT_DEPTH =>
                        {
                            scope.spawn(move |_| {
                                traverse_new_cells(
                                    child,
                                    next_merkle_depth,
                                    next_traverse_depth,
                                    None,
                                    visited,
                                    old_cells,
                                );
                            });
                        }
                        _ => {
                            traverse_new_cells(
                                child,
                                next_merkle_depth,
                                next_traverse_depth,
                                scope,
                                visited,
                                old_cells,
                            );
                        }
                    }
                }

                debug_assert_eq!(
                    merkle_depth,
                    next_merkle_depth - iter.cell().descriptor().is_merkle() as u8,
                );
            }

            let merkle_depth = self.new.descriptor().is_merkle() as u8;

            traverse_new_cells(
                self.new.as_ref(),
                merkle_depth,
                0,
                Some(scope),
                &visited,
                &old_cells,
            )
        });

        Ok(old_cells)
    }
}

/// Helper struct to build a Merkle update.
pub struct MerkleUpdateBuilder<'a, F> {
    old: &'a DynCell,
    new: &'a DynCell,
    filter: F,
}

impl<'a, F> MerkleUpdateBuilder<'a, F>
where
    F: MerkleFilter,
{
    /// Creates a new Merkle update between the specified cells,
    /// using old cells determined by filter.
    pub fn new(old: &'a DynCell, new: &'a DynCell, f: F) -> Self {
        Self {
            old,
            new,
            filter: f,
        }
    }

    /// Builds a Merkle update using the specified cell context.
    pub fn build_ext(self, context: &dyn CellContext) -> Result<MerkleUpdate, Error> {
        BuilderImpl {
            old: self.old,
            new: self.new,
            filter: &self.filter,
            context,
        }
        .build()
    }

    /// Builds a Merkle update using an empty cell context.
    pub fn build(self) -> Result<MerkleUpdate, Error> {
        self.build_ext(Cell::empty_context())
    }
}

#[cfg(all(feature = "rayon", feature = "sync"))]
impl<'a, F> MerkleUpdateBuilder<'a, F>
where
    F: MerkleFilter + Send + Sync,
{
    /// Multithread build of a Merkle update using the specified cell context
    /// and sets of cells which to handle in parallel.
    pub fn par_build_ext(
        self,
        old_split_at: ahash::HashSet<HashBytes>,
        new_split_at: ahash::HashSet<HashBytes>,
        context: &(dyn CellContext + Send + Sync),
    ) -> Result<MerkleUpdate, Error> {
        ParBuilderImpl {
            old: self.old,
            new: self.new,
            filter: &self.filter,
            context,
        }
        .build(old_split_at, new_split_at)
    }

    /// Multithread build of a Merkle update using the default cell context
    /// and sets of cells which to handle in parallel.
    pub fn par_build(
        self,
        old_split_at: ahash::HashSet<HashBytes>,
        new_split_at: ahash::HashSet<HashBytes>,
    ) -> Result<MerkleUpdate, Error> {
        self.par_build_ext(old_split_at, new_split_at, Cell::empty_context())
    }
}

struct BuilderImpl<'a, 'b, 'c: 'a> {
    old: &'a DynCell,
    new: &'a DynCell,
    filter: &'b dyn MerkleFilter,
    context: &'c dyn CellContext,
}

impl<'a: 'b, 'b, 'c: 'a> BuilderImpl<'a, 'b, 'c> {
    fn build(self) -> Result<MerkleUpdate, Error> {
        struct Resolver<'a> {
            pruned_branches: ahash::HashSet<&'a HashBytes>,
            visited: ahash::HashMap<&'a HashBytes, bool>,
            filter: &'a dyn MerkleFilter,
            changed_cells: ahash::HashSet<&'a HashBytes>,
        }

        impl<'a> Resolver<'a> {
            fn fill(&mut self, cell: &'a DynCell, mut skip_filter: bool) -> bool {
                let repr_hash = cell.repr_hash();

                // Skip visited cells
                if let Some(&result) = self.visited.get(repr_hash) {
                    return result;
                }

                let is_pruned = self.pruned_branches.contains(repr_hash);
                let process_children = if skip_filter {
                    true
                } else {
                    match self.filter.check(repr_hash) {
                        FilterAction::Skip => false,
                        FilterAction::Include => true,
                        FilterAction::IncludeSubtree => {
                            skip_filter = true;
                            true
                        }
                    }
                };

                let mut result = false;
                if process_children {
                    for child in cell.references() {
                        result |= self.fill(child, skip_filter);
                    }

                    if result {
                        self.changed_cells.insert(repr_hash);
                    }
                }

                result |= is_pruned;

                self.visited.insert(repr_hash, result);

                result
            }
        }

        struct InvertedFilter<F>(F);

        impl<F: MerkleFilter> MerkleFilter for InvertedFilter<F> {
            #[inline]
            fn check(&self, cell: &HashBytes) -> FilterAction {
                if self.0.check(cell) == FilterAction::Skip {
                    // TODO: check if FilterAction::IncludeSubtree is correct,
                    // because it is more optimal to just include the new subtree
                    FilterAction::Include
                } else {
                    FilterAction::Skip
                }
            }
        }

        let old_hash = self.old.repr_hash();
        let old_depth = self.old.repr_depth();
        let new_hash = self.new.repr_hash();
        let new_depth = self.new.repr_depth();

        // Handle the simplest case with empty Merkle update
        if old_hash == new_hash {
            let pruned = ok!(make_pruned_branch(self.old, 0, self.context));
            return Ok(MerkleUpdate {
                old_hash: *old_hash,
                new_hash: *old_hash,
                old_depth,
                new_depth: old_depth,
                old: pruned.clone(),
                new: pruned,
            });
        }

        // Create Merkle proof cell which contains only new cells
        let (new, pruned_branches) = ok! {
            MerkleProofBuilder::<_>::new(
                self.new,
                InvertedFilter(self.filter)
            )
            .track_pruned_branches()
            .allow_different_root(true)
            .build_raw_ext(self.context)
        };

        let changed_cells = {
            // Prepare cell diff resolver
            let mut resolver = Resolver {
                pruned_branches,
                visited: Default::default(),
                filter: self.filter,
                changed_cells: Default::default(),
            };

            // Find all changed cells in the old cell tree
            if resolver.fill(self.old, false) {
                resolver.changed_cells.insert(old_hash);
            }

            resolver.changed_cells
        };

        // Create Merkle proof cell which contains only changed cells
        let old = ok! {
            MerkleProofBuilder::<_>::new(self.old, changed_cells)
                .allow_different_root(true)
                .build_raw_ext(self.context)
        };

        // Done
        Ok(MerkleUpdate {
            old_hash: *old_hash,
            new_hash: *new_hash,
            old_depth,
            new_depth,
            old,
            new,
        })
    }
}

#[cfg(all(feature = "rayon", feature = "sync"))]
struct ParBuilderImpl<'a, 'b, 'c: 'a> {
    old: &'a DynCell,
    new: &'a DynCell,
    filter: &'b (dyn MerkleFilter + Send + Sync),
    context: &'c (dyn CellContext + Send + Sync),
}

#[cfg(all(feature = "rayon", feature = "sync"))]
impl<'a: 'b, 'b, 'c: 'a> ParBuilderImpl<'a, 'b, 'c> {
    fn build(
        self,
        old_split_at: ahash::HashSet<HashBytes>,
        new_split_at: ahash::HashSet<HashBytes>,
    ) -> Result<MerkleUpdate, Error> {
        enum CheckResult<'a> {
            Immediate(bool),
            Parts {
                hash: &'a HashBytes,
                items: Vec<CheckResult<'a>>,
                is_pruned: bool,
            },
            Deferred(Promise<bool>),
        }

        struct Resolver<'a> {
            pruned_branches: dashmap::DashSet<&'a HashBytes, ahash::RandomState>,
            changed_cells: dashmap::DashSet<&'a HashBytes, ahash::RandomState>,
            deferred: dashmap::DashMap<&'a HashBytes, Promise<bool>, ahash::RandomState>,
            filter: &'a (dyn MerkleFilter + Send + Sync),
        }

        impl<'a> Resolver<'a> {
            fn fill<'s>(&self, cell: &'a DynCell, split_at: &'s ahash::HashSet<HashBytes>) -> bool {
                let result = rayon::scope(|scope| {
                    SubResolver {
                        resolver: self,
                        split_at,
                        visited: Default::default(),
                    }
                    .fill_impl(cell, false, Some(scope))
                });
                self.finalize(result)
            }

            fn finalize(&self, result: CheckResult<'a>) -> bool {
                match result {
                    CheckResult::Immediate(result) => result,
                    CheckResult::Parts {
                        hash,
                        items,
                        is_pruned,
                    } => {
                        let mut result = false;
                        for item in items {
                            result |= self.finalize(item);
                        }

                        if result {
                            self.changed_cells.insert(hash);
                        }

                        result | is_pruned
                    }
                    CheckResult::Deferred(promise) => promise.wait_cloned(),
                }
            }
        }

        struct SubResolver<'a, 's, 'r> {
            resolver: &'r Resolver<'a>,
            split_at: &'s ahash::HashSet<HashBytes>,
            visited: ahash::HashMap<&'a HashBytes, bool>,
        }

        impl<'a, 's, 'r> SubResolver<'a, 's, 'r> {
            fn fill_impl<'scope>(
                &mut self,
                cell: &'a DynCell,
                mut skip_filter: bool,
                scope: Option<&rayon::Scope<'scope>>,
            ) -> CheckResult<'a>
            where
                'a: 'scope,
                's: 'scope,
                'r: 'scope,
            {
                let repr_hash = cell.repr_hash();

                // Skip visited cells
                if let Some(&result) = self.visited.get(repr_hash) {
                    return CheckResult::Immediate(result);
                }

                let is_pruned = self.resolver.pruned_branches.contains(repr_hash);
                let process_children = if skip_filter {
                    true
                } else {
                    match self.resolver.filter.check(repr_hash) {
                        FilterAction::Skip => false,
                        FilterAction::Include => true,
                        FilterAction::IncludeSubtree => {
                            skip_filter = true;
                            true
                        }
                    }
                };

                if !process_children {
                    self.visited.insert(repr_hash, is_pruned);
                    return CheckResult::Immediate(is_pruned);
                }

                if let Some(scope) = scope {
                    if unlikely(self.split_at.contains(repr_hash)) {
                        let entry = match self.resolver.deferred.entry(repr_hash) {
                            dashmap::Entry::Occupied(entry) => {
                                return CheckResult::Deferred(entry.get().clone());
                            }
                            dashmap::Entry::Vacant(entry) => entry,
                        };

                        let promise = Promise::new();
                        scope.spawn({
                            let promise = promise.clone();
                            let resolver = self.resolver;
                            let split_at = self.split_at;
                            move |_| {
                                let CheckResult::Immediate(changed) = SubResolver {
                                    resolver,
                                    split_at,
                                    visited: Default::default(),
                                }
                                .process_children_impl(cell, skip_filter, is_pruned, None) else {
                                    unreachable!("deferred tasks will not spawn");
                                };
                                promise.set(changed)
                            }
                        });
                        return CheckResult::Deferred(entry.insert_entry(promise).get().clone());
                    }
                }

                let result = self.process_children_impl(cell, skip_filter, is_pruned, scope);

                // Mark as visited only finished cells
                if let CheckResult::Immediate(has_changed) = result {
                    self.visited.insert(repr_hash, has_changed);
                }

                result
            }

            #[inline]
            fn process_children_impl<'scope>(
                &mut self,
                cell: &'a DynCell,
                skip_filter: bool,
                is_pruned: bool,
                scope: Option<&rayon::Scope<'scope>>,
            ) -> CheckResult<'a>
            where
                'a: 'scope,
                's: 'scope,
                'r: 'scope,
            {
                let cell_hash = cell.repr_hash();
                let mut children = cell.references();

                let mut items = 'immediate: {
                    let mut result = false;
                    for child in children.by_ref() {
                        match self.fill_impl(child, skip_filter, scope) {
                            CheckResult::Immediate(res) => {
                                result |= res;
                            }
                            res => break 'immediate vec![CheckResult::Immediate(result), res],
                        }
                    }

                    if result {
                        self.resolver.changed_cells.insert(cell_hash);
                    }

                    return CheckResult::Immediate(result | is_pruned);
                };

                for child in children {
                    items.push(self.fill_impl(child, skip_filter, scope));
                }

                CheckResult::Parts {
                    hash: cell_hash,
                    items,
                    is_pruned,
                }
            }
        }

        struct InvertedFilter<F>(F);

        impl<F: MerkleFilter + Send + Sync> MerkleFilter for InvertedFilter<F> {
            #[inline]
            fn check(&self, cell: &HashBytes) -> FilterAction {
                if self.0.check(cell) == FilterAction::Skip {
                    // TODO: check if FilterAction::IncludeSubtree is correct,
                    // because it is more optimal to just include the new subtree
                    FilterAction::Include
                } else {
                    FilterAction::Skip
                }
            }
        }

        let old_hash = self.old.repr_hash();
        let old_depth = self.old.repr_depth();
        let new_hash = self.new.repr_hash();
        let new_depth = self.new.repr_depth();

        // Handle the simplest case with empty Merkle update
        if old_hash == new_hash {
            let pruned = ok!(make_pruned_branch(self.old, 0, self.context));
            return Ok(MerkleUpdate {
                old_hash: *old_hash,
                new_hash: *old_hash,
                old_depth,
                new_depth: old_depth,
                old: pruned.clone(),
                new: pruned,
            });
        }

        // Create Merkle proof cell which contains only new cells
        let (new, pruned_branches) = ok! {
            MerkleProofBuilder::<_>::new(
                self.new,
                InvertedFilter(self.filter)
            )
            .track_pruned_branches()
            .allow_different_root(true)
            .par_build_raw_ext(self.context, new_split_at)
        };

        // Prepare cell diff resolver and find all changed cells in the old cell tree.
        let changed_cells = {
            let resolver = Resolver {
                pruned_branches,
                deferred: Default::default(),
                changed_cells: Default::default(),
                filter: self.filter,
            };

            // Find all changed cells in the old cell tree
            if resolver.fill(self.old, &old_split_at) {
                resolver.changed_cells.insert(old_hash);
            }

            resolver.changed_cells
        };

        // Create Merkle proof cell which contains only changed cells
        let old = ok! {
            MerkleProofBuilder::<_>::new(self.old, changed_cells)
                .allow_different_root(true)
                .par_build_raw_ext(self.context, old_split_at)
        };

        // Done
        Ok(MerkleUpdate {
            old_hash: *old_hash,
            new_hash: *new_hash,
            old_depth,
            new_depth,
            old,
            new,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;

    fn visit_all_cells(cell: &Cell) -> ahash::HashSet<&HashBytes> {
        let mut result = ahash::HashSet::default();

        let mut stack = vec![cell.as_ref()];
        while let Some(cell) = stack.pop() {
            let repr_hash = cell.repr_hash();
            if !result.insert(repr_hash) {
                continue;
            }

            for child in cell.references() {
                stack.push(child);
            }
        }

        result
    }

    #[test]
    fn correct_store_load() {
        let default = MerkleUpdate::default();

        let mut builder = CellBuilder::new();
        default
            .store_into(&mut builder, Cell::empty_context())
            .unwrap();
        let cell = builder.build().unwrap();

        let parsed = cell.parse_exotic::<MerkleUpdate>().unwrap();
        assert_eq!(default, parsed);
    }

    #[test]
    fn dict_merkle_update() {
        // Create dict with keys 0..10
        let mut dict = Dict::<u32, u32>::new();
        for i in 0..10 {
            dict.add(i, i * 10).unwrap();
        }

        // Serialize old dict
        let old_dict_cell = CellBuilder::build_from(&dict).unwrap();
        let old_dict_hashes = visit_all_cells(&old_dict_cell);

        // Serialize new dict
        dict.set(0, 1).unwrap();
        let new_dict_cell = CellBuilder::build_from(dict).unwrap();

        assert_ne!(old_dict_cell.as_ref(), new_dict_cell.as_ref());

        // Create merkle update
        let merkle_update = MerkleUpdate::create(
            old_dict_cell.as_ref(),
            new_dict_cell.as_ref(),
            old_dict_hashes,
        )
        .build()
        .unwrap();

        {
            // Test serialization
            let mut builder = CellBuilder::new();
            merkle_update
                .store_into(&mut builder, Cell::empty_context())
                .unwrap();
            builder.build().unwrap();
        }

        let after_apply = merkle_update.apply(&old_dict_cell).unwrap();
        assert_eq!(after_apply.as_ref(), new_dict_cell.as_ref());
    }

    #[test]
    fn dict_merkle_update_with_usage_tree() {
        // Serialize old dict
        let old_dict_cell = {
            // Create dict with keys 0..10
            let mut dict = Dict::<u32, u32>::new();
            for i in 0..10 {
                dict.add(i, i * 10).unwrap();
            }

            CellBuilder::build_from(&dict).unwrap()
        };

        let usage_tree = UsageTree::new(UsageTreeMode::OnLoad);
        let old_dict_cell_tracked = usage_tree.track(&old_dict_cell);

        // Serialize new dict
        let new_dict_cell = {
            let mut dict = old_dict_cell_tracked.parse::<Dict<u32, u32>>().unwrap();
            dict.set(0, 1).unwrap();
            dict.set(10, 321).unwrap();
            CellBuilder::build_from(dict).unwrap()
        };

        assert_ne!(old_dict_cell.as_ref(), new_dict_cell.as_ref());

        // Create merkle update
        let merkle_update =
            MerkleUpdate::create(old_dict_cell.as_ref(), new_dict_cell.as_ref(), usage_tree)
                .build()
                .unwrap();

        // Test serialization
        let merkle_update_cell = CellBuilder::build_from(&merkle_update).unwrap();
        println!("BOC: {}", Boc::encode_base64(merkle_update_cell));

        let after_apply = merkle_update.apply(&old_dict_cell).unwrap();
        assert_eq!(after_apply.as_ref(), new_dict_cell.as_ref());
    }

    #[test]
    fn dict_removed_cells_diff() {
        // Create dict with keys 0..10
        let mut dict = Dict::<u32, u32>::new();
        for i in 0..10 {
            dict.add(i, 0).unwrap();
        }

        // Serialize old dict
        let old_dict_cell = CellBuilder::build_from(&dict).unwrap();
        let old_dict_hashes = visit_all_cells(&old_dict_cell);

        // Serialize new dict
        dict.set(0, 1).unwrap();
        let new_dict_cell = CellBuilder::build_from(dict).unwrap();

        assert_ne!(old_dict_cell.as_ref(), new_dict_cell.as_ref());

        // Create merkle update
        let merkle_update = MerkleUpdate::create(
            old_dict_cell.as_ref(),
            new_dict_cell.as_ref(),
            old_dict_hashes,
        )
        .build()
        .unwrap();

        // Test diff
        let mut refs_for_both = RefsStorage::default();
        refs_for_both.store_cell(old_dict_cell.as_ref());
        refs_for_both.store_cell(new_dict_cell.as_ref());

        let mut only_new_refs = RefsStorage::default();
        only_new_refs.store_cell(new_dict_cell.as_ref());

        let diff = merkle_update
            .compute_removed_cells(old_dict_cell.as_ref())
            .unwrap();
        assert!(refs_for_both.remove_batch(diff));

        assert_eq!(only_new_refs.refs, refs_for_both.refs);
    }

    #[test]
    fn dict_removed_all_cells_diff() {
        // Create dict with keys 0..10
        let mut dict = Dict::<u32, u32>::new();
        for i in 0..10 {
            dict.add(i, 0).unwrap();
        }

        // Serialize old dict
        let old_dict_cell = CellBuilder::build_from(&dict).unwrap();
        let old_dict_hashes = visit_all_cells(&old_dict_cell);

        // Serialize new dict
        let new_dict_cell = CellBuilder::build_from(Dict::<u32, u32>::new()).unwrap();

        assert_ne!(old_dict_cell.as_ref(), new_dict_cell.as_ref());

        // Create merkle update
        let merkle_update = MerkleUpdate::create(
            old_dict_cell.as_ref(),
            new_dict_cell.as_ref(),
            old_dict_hashes,
        )
        .build()
        .unwrap();

        // Test diff
        let mut refs_for_both = RefsStorage::default();
        refs_for_both.store_cell(old_dict_cell.as_ref());
        refs_for_both.store_cell(new_dict_cell.as_ref());

        let mut only_new_refs = RefsStorage::default();
        only_new_refs.store_cell(new_dict_cell.as_ref());

        let diff = merkle_update
            .compute_removed_cells(old_dict_cell.as_ref())
            .unwrap();
        assert!(refs_for_both.remove_batch(diff));

        assert_eq!(only_new_refs.refs, refs_for_both.refs);
    }

    #[derive(Default)]
    struct RefsStorage<'a> {
        refs: ahash::HashMap<&'a HashBytes, u32>,
    }

    impl<'a> RefsStorage<'a> {
        fn store_cell(&mut self, root: &'a DynCell) {
            use std::collections::hash_map;

            *self.refs.entry(root.repr_hash()).or_default() += 1;

            let mut stack = vec![root.references()];
            'outer: while let Some(iter) = stack.last_mut() {
                for child in iter {
                    let hash = child.repr_hash();
                    match self.refs.entry(hash) {
                        hash_map::Entry::Occupied(mut entry) => {
                            *entry.get_mut() += 1;
                            continue;
                        }
                        hash_map::Entry::Vacant(entry) => {
                            entry.insert(1);
                        }
                    }

                    stack.push(child.references());
                    continue 'outer;
                }
                stack.pop();
            }
        }

        fn remove_batch(&mut self, mut batch: ahash::HashMap<&'a HashBytes, u32>) -> bool {
            self.refs.retain(|hash, refs| {
                if let Some(diff) = batch.remove(hash) {
                    *refs -= diff;
                }
                *refs != 0
            });
            batch.is_empty()
        }
    }
}
