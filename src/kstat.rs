use std::ffi::CStr;

use anyhow::{bail, Result};

pub mod consts {
    use std::ffi::CStr;

    pub const MODULE_CPU_INFO: &CStr =
        unsafe { &CStr::from_bytes_with_nul_unchecked(b"cpu_info\0") };

    pub const STAT_CLOCK_MHZ: &CStr =
        unsafe { &CStr::from_bytes_with_nul_unchecked(b"clock_MHz\0") };

    pub const MODULE_UNIX: &CStr =
        unsafe { &CStr::from_bytes_with_nul_unchecked(b"unix\0") };

    pub const NAME_SYSTEM_MISC: &CStr =
        unsafe { &CStr::from_bytes_with_nul_unchecked(b"system_misc\0") };
    pub const STAT_BOOT_TIME: &CStr =
        unsafe { &CStr::from_bytes_with_nul_unchecked(b"boot_time\0") };
    pub const STAT_NPROC: &CStr =
        unsafe { &CStr::from_bytes_with_nul_unchecked(b"nproc\0") };

    pub const NAME_SYSTEM_PAGES: &CStr =
        unsafe { &CStr::from_bytes_with_nul_unchecked(b"system_pages\0") };
    pub const STAT_FREEMEM: &CStr =
        unsafe { &CStr::from_bytes_with_nul_unchecked(b"freemem\0") };
    pub const STAT_PHYSMEM: &CStr =
        unsafe { &CStr::from_bytes_with_nul_unchecked(b"physmem\0") };
    pub const STAT_AVAILRMEM: &CStr =
        unsafe { &CStr::from_bytes_with_nul_unchecked(b"availrmem\0") };

    pub const MODULE_ZFS: &CStr =
        unsafe { &CStr::from_bytes_with_nul_unchecked(b"zfs\0") };
    pub const NAME_ARCSTATS: &CStr =
        unsafe { &CStr::from_bytes_with_nul_unchecked(b"arcstats\0") };
    pub const STAT_C: &CStr =
        unsafe { &CStr::from_bytes_with_nul_unchecked(b"c\0") };
    pub const STAT_C_MIN: &CStr =
        unsafe { &CStr::from_bytes_with_nul_unchecked(b"c_min\0") };
    pub const STAT_C_MAX: &CStr =
        unsafe { &CStr::from_bytes_with_nul_unchecked(b"c_max\0") };
}
use consts::*;

pub use wrapper::KstatWrapper;

#[derive(Debug)]
pub struct KstatDataIo {
    pub nread: u64,
    pub nwritten: u64,
    pub reads: u32,
    pub writes: u32,
    pub wtime: i64,
    pub wlentime: i64,
    pub wlastupdate: i64,
    pub rtime: i64,
    pub rlentime: i64,
    pub rlastupdate: i64,
    pub wcnt: u32,
    pub rcnt: u32,
}

#[derive(Debug)]
pub enum KstatDataValue {
    Char(i8),
    S32(i32),
    U32(u32),
    S64(i64),
    U64(u64),
    Unknown(u8),
}

#[derive(Debug)]
pub struct KstatData {
    pub name: std::ffi::CString,
    pub value: KstatDataValue,
}

mod wrapper {
    use super::{KstatData, KstatDataIo, KstatDataValue};
    use anyhow::{anyhow, bail, Result};
    use std::ffi::CStr;
    use std::os::raw::c_char;
    use std::os::raw::c_int;
    use std::os::raw::c_long;
    use std::os::raw::c_longlong;
    use std::os::raw::c_uchar;
    use std::os::raw::c_uint;
    use std::os::raw::c_ulong;
    use std::os::raw::c_ulonglong;
    use std::os::raw::c_void;
    use std::ptr::{null, null_mut, NonNull};

    const KSTAT_TYPE_NAMED: c_uchar = 1;
    const KSTAT_TYPE_IO: c_uchar = 3;

    const KSTAT_STRLEN: usize = 31;

    const KSTAT_DATA_CHAR: u8 = 0;
    const KSTAT_DATA_INT32: u8 = 1;
    const KSTAT_DATA_UINT32: u8 = 2;
    const KSTAT_DATA_INT64: u8 = 3;
    const KSTAT_DATA_UINT64: u8 = 4;

    #[repr(C)]
    struct Kstat {
        ks_crtime: c_longlong,
        ks_next: *mut Kstat,
        ks_kid: c_uint,
        ks_module: [c_char; KSTAT_STRLEN],
        ks_resv: c_uchar,
        ks_instance: c_int,
        ks_name: [c_char; KSTAT_STRLEN],
        ks_type: c_uchar,
        ks_class: [c_char; KSTAT_STRLEN],
        ks_flags: c_uchar,
        ks_data: *mut c_void,
        ks_ndata: c_uint,
        ks_data_size: usize,
        ks_snaptime: c_longlong,
    }

    impl Kstat {
        fn name(&self) -> &CStr {
            unsafe { CStr::from_ptr(self.ks_name.as_ptr()) }
        }

        fn module(&self) -> &CStr {
            unsafe { CStr::from_ptr(self.ks_module.as_ptr()) }
        }

        fn class(&self) -> &CStr {
            unsafe { CStr::from_ptr(self.ks_class.as_ptr()) }
        }

        fn instance(&self) -> i32 {
            self.ks_instance
        }

        fn type_(&self) -> u8 {
            self.ks_type
        }
    }

    #[repr(C)]
    struct KstatCtl {
        kc_chain_id: c_int,
        kc_chain: *mut Kstat,
        kc_kd: c_int,
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    union KstatValue {
        c: [c_char; 16],
        l: c_long,
        ul: c_ulong,
        ui32: u32,
        si32: i32,
        ui64: u64,
        si64: i64,
    }

    #[repr(C)]
    struct KstatNamed {
        name: [c_char; KSTAT_STRLEN],
        data_type: c_uchar,
        value: KstatValue,
    }

    #[repr(C)]
    pub struct KstatIo {
        pub nread: c_ulonglong,
        pub nwritten: c_ulonglong,
        pub reads: c_uint,
        pub writes: c_uint,
        pub wtime: c_longlong,
        pub wlentime: c_longlong,
        pub wlastupdate: c_longlong,
        pub rtime: c_longlong,
        pub rlentime: c_longlong,
        pub rlastupdate: c_longlong,
        pub wcnt: c_uint,
        pub rcnt: c_uint,
    }

    impl KstatNamed {
        fn name(&self) -> &CStr {
            unsafe { CStr::from_ptr(self.name.as_ptr()) }
        }
    }

    #[link(name = "kstat")]
    extern "C" {
        fn kstat_open() -> *mut KstatCtl;
        fn kstat_close(kc: *mut KstatCtl) -> c_int;
        fn kstat_lookup(
            kc: *mut KstatCtl,
            module: *const c_char,
            instance: c_int,
            name: *const c_char,
        ) -> *mut Kstat;
        fn kstat_read(
            kc: *mut KstatCtl,
            ksp: *mut Kstat,
            buf: *mut c_void,
        ) -> c_int;
        fn kstat_data_lookup(
            ksp: *mut Kstat,
            name: *const c_char,
        ) -> *mut c_void;
        fn kstat_chain_update(ksp: *mut KstatCtl) -> c_int;
    }

    /// Minimal wrapper around libkstat(3LIB) on illumos and Solaris systems.
    pub struct KstatWrapper {
        kc: NonNull<KstatCtl>,
        ks: Option<NonNull<Kstat>>,
        stepping: bool,
    }

    unsafe impl Send for KstatWrapper {}
    unsafe impl Sync for KstatWrapper {}

    /// Turn an optional CStr into a (const char *) for Some, or NULL for None.
    fn cp(p: &Option<&CStr>) -> *const c_char {
        p.map_or_else(|| null(), |p| p.as_ptr())
    }

    impl KstatWrapper {
        pub fn open() -> Result<Self> {
            let kc = NonNull::new(unsafe { kstat_open() });
            if let Some(kc) = kc {
                Ok(KstatWrapper { kc: kc, ks: None, stepping: false })
            } else {
                let e = std::io::Error::last_os_error();
                Err(anyhow!("kstat_open(3KSTAT) failed: {}", e))
            }
        }

        /// Call kstat_chain_update(3KSTAT).  A new lookup() or walk() must
        /// be performed after calling this function, as the chain update
        /// invalidates any active pointers and memory.
        pub fn chain_update(&mut self) -> Result<()> {
            /*
             * First, clear out the existing walk if one is in progress.
             */
            self.stepping = false;
            self.ks = None;

            if unsafe { kstat_chain_update(self.kc.as_ptr()) } == -1 {
                bail!(
                    "kstat_chain_update() failure: {}",
                    std::io::Error::last_os_error()
                );
            }

            Ok(())
        }

        /// Call kstat_lookup(3KSTAT) and store the result, if there is a match.
        pub fn lookup(&mut self, module: Option<&CStr>, name: Option<&CStr>) {
            self.ks = NonNull::new(unsafe {
                kstat_lookup(self.kc.as_ptr(), cp(&module), -1, cp(&name))
            });

            self.stepping = false;
        }

        /// Start a new walk of the kstat chain from the beginning.
        pub fn walk(&mut self) {
            self.ks = NonNull::new(unsafe { self.kc.as_ref().kc_chain });

            self.stepping = false;
        }

        /// Call once to start iterating, and then repeatedly for each
        /// additional kstat in the chain.  Returns false once there are no more
        /// kstat entries.
        pub fn step(&mut self) -> bool {
            if !self.stepping {
                self.stepping = true;
            } else {
                self.ks = self.ks.map_or(None, |ks| {
                    NonNull::new(unsafe { ks.as_ref() }.ks_next)
                });
            }

            if self.ks.is_none() {
                self.stepping = false;
                false
            } else {
                true
            }
        }

        /// Return the module name of the current kstat.  This routine will
        /// panic if step() has not returned true.
        pub fn module(&self) -> &CStr {
            let ks = self.ks.as_ref().expect("step() must return true first");
            unsafe { ks.as_ref() }.module()
        }

        /// Return the name of the current kstat.  This routine will panic if
        /// step() has not returned true.
        pub fn name(&self) -> &CStr {
            let ks = self.ks.as_ref().expect("step() must return true first");
            unsafe { ks.as_ref() }.name()
        }

        /// Return the class of the current kstat.  This routine will panic if
        /// step() has not returned true.
        pub fn class(&self) -> &CStr {
            let ks = self.ks.as_ref().expect("step() must return true first");
            unsafe { ks.as_ref() }.class()
        }

        /// Return the class of the current kstat.  This routine will panic if
        /// step() has not returned true.
        pub fn instance(&self) -> i32 {
            let ks = self.ks.as_ref().expect("step() must return true first");
            unsafe { ks.as_ref() }.instance()
        }

        /// Return the class of the current kstat.  This routine will panic if
        /// step() has not returned true.
        pub fn type_(&self) -> u8 {
            let ks = self.ks.as_ref().expect("step() must return true first");
            unsafe { ks.as_ref() }.type_()
        }

        pub fn read(&self) -> Result<()> {
            let ksp = if let Some(ks) = &self.ks {
                ks.as_ptr()
            } else {
                bail!("no kstat_t");
            };

            if unsafe { kstat_read(self.kc.as_ptr(), ksp, null_mut()) } == -1 {
                bail!(
                    "kstat_read() failure: {}",
                    std::io::Error::last_os_error()
                );
            }

            Ok(())
        }

        pub fn ndata(&self) -> usize {
            let ks = if let Some(ks) = &self.ks {
                unsafe { ks.as_ref() }
            } else {
                return 0;
            };

            if ks.ks_type != KSTAT_TYPE_NAMED {
                // This is not a named kstat
                0
            } else {
                ks.ks_ndata as usize
            }
        }

        pub fn io(&self) -> Option<KstatDataIo> {
            let ks = if let Some(ks) = &self.ks {
                unsafe { ks.as_ref() }
            } else {
                return None;
            };

            if ks.ks_type != KSTAT_TYPE_IO {
                return None;
            }

            let ksd: NonNull<KstatIo> =
                NonNull::new(ks.ks_data).unwrap().cast();

            let ksd = unsafe { ksd.as_ref() };

            Some(KstatDataIo {
                nread: ksd.nread,
                nwritten: ksd.nwritten,
                reads: ksd.reads,
                writes: ksd.writes,
                wtime: ksd.wtime,
                wlentime: ksd.wlentime,
                wlastupdate: ksd.wlastupdate,
                rtime: ksd.rtime,
                rlentime: ksd.rlentime,
                rlastupdate: ksd.rlastupdate,
                wcnt: ksd.wcnt,
                rcnt: ksd.rcnt,
            })
        }

        pub fn data_get(&self, n: usize) -> Option<KstatData> {
            let ks = if let Some(ks) = &self.ks {
                unsafe { ks.as_ref() }
            } else {
                return None;
            };

            if ks.ks_type != KSTAT_TYPE_NAMED || n >= ks.ks_ndata as usize {
                // This is not a named kstat, or it does not have this many
                // data elements.
                return None;
            }

            let ksd = NonNull::new(ks.ks_data).unwrap().cast();

            let data: &[KstatNamed] = unsafe {
                std::slice::from_raw_parts(ksd.as_ptr(), ks.ks_ndata as usize)
            };

            let value = match data[n].data_type {
                KSTAT_DATA_CHAR => {
                    KstatDataValue::Char(unsafe { data[n].value.c[0] })
                }
                KSTAT_DATA_INT32 => {
                    KstatDataValue::S32(unsafe { data[n].value.si32 })
                }
                KSTAT_DATA_INT64 => {
                    KstatDataValue::S64(unsafe { data[n].value.si64 })
                }
                KSTAT_DATA_UINT32 => {
                    KstatDataValue::U32(unsafe { data[n].value.ui32 })
                }
                KSTAT_DATA_UINT64 => {
                    KstatDataValue::U64(unsafe { data[n].value.ui64 })
                }
                n => KstatDataValue::Unknown(n),
            };

            Some(KstatData { name: data[n].name().to_owned(), value })
        }

        /// Look up a named kstat value.  For internal use by typed accessors.
        fn data_value(&self, statistic: &CStr) -> Option<NonNull<KstatNamed>> {
            let (ks, ksp) = if let Some(ks) = &self.ks {
                (unsafe { ks.as_ref() }, ks.as_ptr())
            } else {
                return None;
            };

            if unsafe { kstat_read(self.kc.as_ptr(), ksp, null_mut()) } == -1 {
                return None;
            }

            if ks.ks_type != KSTAT_TYPE_NAMED || ks.ks_ndata < 1 {
                // This is not a named kstat, or it has no data payload.
                return None;
            }

            NonNull::new(unsafe {
                kstat_data_lookup(ksp, cp(&Some(statistic)))
            })
            .map(|voidp| voidp.cast())
        }

        /// Look up a named kstat value and interpret it as a "long_t".
        pub fn data_long(&self, statistic: &CStr) -> Option<i64> {
            self.data_value(statistic)
                .map(|kn| unsafe { kn.as_ref().value.l } as i64)
        }

        /// Look up a named kstat value and interpret it as a "ulong_t".
        pub fn data_ulong(&self, statistic: &CStr) -> Option<u64> {
            self.data_value(statistic)
                .map(|kn| unsafe { kn.as_ref().value.ul } as u64)
        }

        /// Look up a named kstat value and interpret it as a "uint32_t".
        pub fn data_u32(&self, statistic: &CStr) -> Option<u32> {
            self.data_value(statistic)
                .map(|kn| unsafe { kn.as_ref().value.ui32 })
        }

        /// Look up a named kstat value and interpret it as a "uint64_t".
        pub fn data_u64(&self, statistic: &CStr) -> Option<u64> {
            self.data_value(statistic)
                .map(|kn| unsafe { kn.as_ref().value.ui64 })
        }
    }

    impl Drop for KstatWrapper {
        fn drop(&mut self) {
            unsafe { kstat_close(self.kc.as_ptr()) };
        }
    }
}

pub fn cpu_mhz() -> Result<u64> {
    let mut k = wrapper::KstatWrapper::open()?;

    k.lookup(Some(MODULE_CPU_INFO), None);
    while k.step() {
        if k.module() != MODULE_CPU_INFO {
            continue;
        }

        if let Some(mhz) = k.data_long(STAT_CLOCK_MHZ) {
            return Ok(mhz as u64);
        }
    }

    bail!("cpu speed kstat not found");
}

pub fn boot_time() -> Result<u64> {
    let mut k = wrapper::KstatWrapper::open()?;

    k.lookup(Some(MODULE_UNIX), Some(NAME_SYSTEM_MISC));
    while k.step() {
        if k.module() != MODULE_UNIX || k.name() != NAME_SYSTEM_MISC {
            continue;
        }

        if let Some(boot_time) = k.data_u32(STAT_BOOT_TIME) {
            return Ok(boot_time as u64);
        }
    }

    bail!("boot time kstat not found");
}

pub fn nproc() -> Result<u64> {
    let mut k = wrapper::KstatWrapper::open()?;

    k.lookup(Some(MODULE_UNIX), Some(NAME_SYSTEM_MISC));
    while k.step() {
        if k.module() != MODULE_UNIX || k.name() != NAME_SYSTEM_MISC {
            continue;
        }

        if let Some(nproc) = k.data_u32(STAT_NPROC) {
            return Ok(nproc as u64);
        }
    }

    bail!("process count kstat not found");
}

pub struct Pages {
    pub freemem: u64,
    pub physmem: u64,
}

pub fn pages() -> Result<Pages> {
    let mut k = wrapper::KstatWrapper::open()?;

    k.lookup(Some(MODULE_UNIX), Some(NAME_SYSTEM_PAGES));
    while k.step() {
        if k.module() != MODULE_UNIX || k.name() != NAME_SYSTEM_PAGES {
            continue;
        }

        let freemem = k.data_ulong(STAT_FREEMEM);
        let physmem = k.data_ulong(STAT_PHYSMEM);

        if freemem.is_some() && physmem.is_some() {
            return Ok(Pages {
                freemem: freemem.unwrap(),
                physmem: physmem.unwrap(),
            });
        }
    }

    bail!("system pages kstat not available");
}
