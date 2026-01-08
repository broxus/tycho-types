use std::hash::{BuildHasher, BuildHasherDefault, Hash, Hasher};

/// A key to be used when working with [`HashBytes`].
///
/// NOTE: DO NOT TRY to implement `Borrow<HashBytesKey>` for [`HashBytes`].
/// `HashMap` will use the hash of some `Q` instead of a [`HashBytesKey`],
/// so the hasher will be used with a wrong input. Always use [`HashBytes::as_key`]
/// for that.
///
/// [`HashBytes`]: crate::cell::HashBytes
/// [`HashBytes::as_key`]: crate::cell::HashBytes::as_key
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
#[repr(transparent)]
pub struct HashBytesKey(pub [u8; 32]);

impl HashBytesKey {
    /// Wraps a reference to an internal array into a newtype reference.
    #[inline(always)]
    pub const fn wrap(value: &[u8; 32]) -> &Self {
        // SAFETY: HashBytes is #[repr(transparent)]
        unsafe { &*(value as *const [u8; 32] as *const Self) }
    }
}

impl Hash for HashBytesKey {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Compiles into something like this:
        // `mov rax, qword ptr [rdi]`
        state.write_u64(u64::from_le_bytes(self.0[0..8].try_into().unwrap()))
    }
}

/// An alias for [`BuildHasherDefault`] for use with [`TrustedCellHasher`].
pub type BuildTrustedCellHasher = BuildHasherDefault<TrustedCellHasher>;

/// A hasher to use for [`HashBytesKey`] when keys are trusted repr hashes.
///
/// Isn't HashDoS resistant at all, so use at your own risk.
#[derive(Default, Debug, Clone, Copy)]
pub struct TrustedCellHasher(u64);

impl Hasher for TrustedCellHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }

    #[inline]
    fn write(&mut self, _bytes: &[u8]) {
        unreachable!("Invalid use of TrustedCellHasher")
    }

    #[inline]
    fn write_u64(&mut self, n: u64) {
        self.0 = n
    }
}

/// A hasher to use for [`HashBytesKey`] when keys are untrusted repr hashes.
///
/// Currently its implementation is based on a specialized
/// [`AHasherU64`](https://github.com/tkaitchuck/aHash/blob/a9d649d18d6aefeef106a48252dc6708ee7f9e47/src/fallback_hash.rs#L201-L250).
#[derive(Debug, Clone)]
pub struct CellHasher {
    buffer: u64,
    pad: u64,
}

impl CellHasher {
    #[inline]
    const fn from_random_state(rand_state: &BuildCellHasher) -> Self {
        Self {
            buffer: rand_state.k1,
            pad: rand_state.k0,
        }
    }
}

/// Provides a default [`Hasher`] with fixed keys.
/// This is typically used in conjunction with [`BuildHasherDefault`] to create
/// [`CellHasher`]s in order to hash the keys of the map.
///
/// Generally it is preferable to use [`BuildCellHasher`] instead,
/// so that different hashmaps will have different keys.
/// However if fixed keys are desirable this may be used instead.
impl Default for CellHasher {
    fn default() -> Self {
        BuildCellHasher::with_fixed_keys().build_hasher()
    }
}

impl Hasher for CellHasher {
    #[inline]
    fn finish(&self) -> u64 {
        ahash_reimpl::folded_multiply(self.buffer, self.pad)
    }

    #[inline]
    fn write(&mut self, _bytes: &[u8]) {
        unreachable!("Invalid use of CellHasher write")
    }

    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.buffer = ahash_reimpl::folded_multiply(i ^ self.buffer, ahash_reimpl::MULTIPLE);
    }
}

/// Provides a [CellHasher] factory.
#[derive(Clone)]
pub struct BuildCellHasher {
    k0: u64,
    k1: u64,
}

impl BuildCellHasher {
    /// Create a new [`BuildCellHasher`] [`BuildHasher`] using random keys.
    ///
    /// Each instance will have a unique set of keys derived from [`RandomSource`].
    ///
    /// [`BuildHasher`]: std::hash::BuildHasher
    /// [`RandomSource`]: ahash::random_state::RandomSource
    #[inline]
    pub fn new() -> Self {
        let src = ahash_reimpl::get_src();
        let fixed = ahash_reimpl::get_fixed_seeds();
        Self::from_keys(&fixed[0], &fixed[1], src.gen_hasher_seed())
    }

    /// Build a [`BuildCellHasher`] from a single key. The provided key does not need
    /// to be of high quality, but all `BuildCellHasher`s created from the same key
    /// will produce identical hashers.
    ///
    /// This allows for explicitly setting the seed to be used.
    ///
    /// Note: This method does not require the provided seed to be strong.
    #[inline]
    pub fn with_seed(key: usize) -> Self {
        let fixed = ahash_reimpl::get_fixed_seeds();
        Self::from_keys(&fixed[0], &fixed[1], key)
    }

    /// Internal. Used by Default.
    #[inline]
    fn with_fixed_keys() -> Self {
        let [k0, k1, ..] = ahash_reimpl::get_fixed_seeds()[0];
        Self { k0, k1 }
    }

    fn from_keys(a: &[u64; 4], b: &[u64; 4], c: usize) -> Self {
        let &[k0, k1, _, _] = a;
        let mut hasher = CellHasher::from_random_state(&Self { k0, k1 });
        hasher.write_u64(c as u64);
        let mix = |l: u64, r: u64| {
            let mut h = hasher.clone();
            h.write_u64(l);
            h.write_u64(r);
            h.finish()
        };
        Self {
            k0: mix(b[0], b[2]),
            k1: mix(b[1], b[3]),
        }
    }

    /// Calculates the hash of a single value.
    #[inline]
    pub fn hash_one(&self, value: &HashBytesKey) -> u64 {
        let mut hasher = CellHasher::from_random_state(self);
        value.hash(&mut hasher);
        hasher.finish()
    }
}

impl Default for BuildCellHasher {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for BuildCellHasher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad("BuildCellHasher { .. }")
    }
}

impl BuildHasher for BuildCellHasher {
    type Hasher = CellHasher;

    #[inline]
    fn build_hasher(&self) -> Self::Hasher {
        CellHasher::from_random_state(self)
    }
}

// Part of aHash implementation:
// https://github.com/tkaitchuck/aHash/blob/a9d649d18d6aefeef106a48252dc6708ee7f9e47/src/random_state.rs
// https://github.com/tkaitchuck/aHash/blob/a9d649d18d6aefeef106a48252dc6708ee7f9e47/src/operations.rs#L5-L28
mod ahash_reimpl {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use ahash::random_state::RandomSource;

    const PI: [u64; 4] = [
        0x243f_6a88_85a3_08d3,
        0x1319_8a2e_0370_7344,
        0xa409_3822_299f_31d0,
        0x082e_fa98_ec4e_6c89,
    ];

    #[allow(unused)]
    const PI2: [u64; 4] = [
        0x4528_21e6_38d0_1377,
        0xbe54_66cf_34e9_0c6c,
        0xc0ac_29b7_c97c_50dd,
        0x3f84_d5b5_b547_0917,
    ];

    cfg_if::cfg_if! {
        if #[cfg(not(all(target_arch = "arm", target_os = "none")))] {
            static RAND_SOURCE: once_cell::race::OnceBox<Box<dyn RandomSource + Send + Sync>> =
                once_cell::race::OnceBox::new();
        }
    }

    cfg_if::cfg_if! {
        if #[cfg(not(fuzzing))] {
            #[inline]
            pub fn get_fixed_seeds() -> &'static [[u64; 4]; 2] {
                static SEEDS: once_cell::race::OnceBox<[[u64; 4]; 2]> = once_cell::race::OnceBox::new();

                SEEDS.get_or_init(|| {
                    let mut result: [u8; 64] = [0; 64];
                    getrandom::fill(&mut result).expect("getrandom::fill() failed.");
                    Box::new(zerocopy::transmute!(result))
                })
            }
        } else {
            #[inline]
            pub fn get_fixed_seeds() -> &'static [[u64; 4]; 2] {
                &[PI, PI2]
            }
        }
    }

    struct DefaultRandomSource {
        counter: AtomicUsize,
    }

    impl DefaultRandomSource {
        fn new() -> Self {
            Self {
                counter: AtomicUsize::new(&PI as *const _ as usize),
            }
        }

        #[cfg(all(target_arch = "arm", target_os = "none"))]
        const fn default() -> Self {
            Self {
                counter: AtomicUsize::new(PI[3] as usize),
            }
        }
    }

    impl RandomSource for DefaultRandomSource {
        cfg_if::cfg_if! {
            if #[cfg(all(target_arch = "arm", target_os = "none"))] {
                fn gen_hasher_seed(&self) -> usize {
                    let stack = self as *const _ as usize;
                    let previous = self.counter.load(Ordering::Relaxed);
                    let new = previous.wrapping_add(stack);
                    self.counter.store(new, Ordering::Relaxed);
                    new
                }
            } else {
                fn gen_hasher_seed(&self) -> usize {
                    let stack = self as *const _ as usize;
                    self.counter.fetch_add(stack, Ordering::Relaxed)
                }
            }
        }
    }

    cfg_if::cfg_if! {
        if #[cfg(all(target_arch = "arm", target_os = "none"))] {
            #[inline]
            pub fn get_src() -> &'static dyn RandomSource {
                static RAND_SOURCE: DefaultRandomSource = DefaultRandomSource::default();
                &RAND_SOURCE
            }
        } else {
            #[inline]
            pub fn get_src() -> &'static dyn RandomSource {
                RAND_SOURCE.get_or_init(|| Box::new(Box::new(DefaultRandomSource::new()))).as_ref()
            }
        }
    }

    pub const MULTIPLE: u64 = 6364136223846793005;

    #[inline(always)]
    #[cfg(folded_multiply)]
    pub const fn folded_multiply(s: u64, by: u64) -> u64 {
        let result = (s as u128).wrapping_mul(by as u128);
        ((result & 0xffff_ffff_ffff_ffff) as u64) ^ ((result >> 64) as u64)
    }

    #[inline(always)]
    #[cfg(not(folded_multiply))]
    pub const fn folded_multiply(s: u64, by: u64) -> u64 {
        let b1 = s.wrapping_mul(by.swap_bytes());
        let b2 = s.swap_bytes().wrapping_mul(!by);
        b1 ^ b2.swap_bytes()
    }
}
