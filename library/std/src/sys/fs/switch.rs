use crate::cmp;
use crate::ffi::{OsString, CStr, OsStr};
use crate::os::switch::ffi::OsStrExt;
use crate::fmt;
use crate::hash::Hash;
use crate::io::{self, IoSlice, IoSliceMut, SeekFrom, BorrowedCursor};
use crate::path::{Path, PathBuf};
use crate::sys::time::{SystemTime, UNIX_EPOCH};
use crate::sync::atomic::{AtomicU64, Ordering};
pub use crate::sys::fs::common::{Dir, exists};
use crate::sys::{ cvt, unsupported, unsupported_err };
use crate::fs::TryLockError;

use nnsdk::fs::{FileHandle, DirectoryEntry as NinDirEntry};
use nnsdk::fs::DirectoryEntryType_DirectoryEntryType_Directory as NN_ENTRY_DIR;
use nnsdk::fs::DirectoryEntryType_DirectoryEntryType_File as NN_ENTRY_FILE;
use nnsdk::time::PosixTime;
use crate::ffi::CString;

macro_rules! r_try {
    ($expr:expr) => {
        {
            let rc = $expr;
            match rc {
                0 => Ok(()),
                0x202 => Err(io::Error::new(io::ErrorKind::NotFound, "the file was not found")),
                rc => Err(io::Error::from_raw_os_error(rc as _))
            }
        }
    };
}

#[derive(Debug)]
pub struct FileAttr {
    size: AtomicU64,
    file_type: FileType,
}

pub struct ReadDir {
    inner_iter: crate::vec::IntoIter<NinDirEntry>,
    root: PathBuf,
}

pub struct DirEntry {
    path: PathBuf,
    size: u64,
    file_type: FileType,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FilePermissions {
    read_only: bool
}

#[derive(Copy, Clone)]
pub struct FileTimes([libc::timespec; 2]);

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum FileType {
    Dir,
    File
}

#[derive(Debug)]
pub struct DirBuilder {}

const READ_LIMIT: usize = libc::ssize_t::MAX as usize;

impl Clone for FileAttr {
    fn clone(&self) -> Self {
        Self {
            size: AtomicU64::new(self.size.load(Ordering::SeqCst)),
            file_type: self.file_type,
        }
    }
}

impl FileAttr {
    pub fn size(&self) -> u64 {
        self.size.load(Ordering::SeqCst)
    }

    pub fn set_size(&self, size: u64) {
        self.size.store(size, Ordering::SeqCst);
    }

    pub fn perm(&self) -> FilePermissions {
        FilePermissions { read_only: false }
    }

    pub fn file_type(&self) -> FileType {
        self.file_type
    }

    pub fn modified(&self) -> io::Result<SystemTime> {
        Ok(UNIX_EPOCH)
    }

    pub fn accessed(&self) -> io::Result<SystemTime> {
        Ok(UNIX_EPOCH)

    }

    pub fn created(&self) -> io::Result<SystemTime> {
        Ok(UNIX_EPOCH)

    }
}

impl FilePermissions {
    pub fn readonly(&self) -> bool {
        self.read_only
    }

    pub fn set_readonly(&mut self, readonly: bool) {
        self.read_only = readonly;
    }
}

impl FileTimes {
    pub fn set_accessed(&mut self, t: SystemTime) {
        unimplemented!()
    }

    pub fn set_modified(&mut self, t: SystemTime) {
        unimplemented!()
    }
}

impl fmt::Debug for FileTimes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileTimes is unimplemented")
            .finish()
    }
}

impl Default for FileTimes {
    fn default() -> Self {
        let omit = libc::timespec { tv_sec: 0, tv_nsec: 0 as _ };
        Self([omit; 2])
    }
}

impl FileType {
    pub fn is_dir(&self) -> bool {
        if let FileType::Dir = self {
            true
        } else {
            false
        }
    }

    pub fn is_file(&self) -> bool {
        if let FileType::File = self {
            true
        } else {
            false
        }
    }

    pub fn is_symlink(&self) -> bool {
        false
    }
}

impl fmt::Debug for ReadDir {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[Directory]")
    }
}

impl Iterator for ReadDir {
    type Item = io::Result<DirEntry>;

    fn next(&mut self) -> Option<io::Result<DirEntry>> {
        let val = self.inner_iter.next()?;

        let len = unsafe { libc::strlen(val.name.as_ptr()) };
        let path = self.root.join(Path::new(OsStr::from_bytes(&val.name[..len])));

        let file_type = match val.type_ as u32 {
            NN_ENTRY_DIR => FileType::Dir,
            NN_ENTRY_FILE => FileType::File,
            _ => panic!("Invalid entry type in directory")
        };
        
        let size = val.fileSize as u64;

        Some(Ok(DirEntry {
            path, size, file_type
        }))
    }
}

impl DirEntry {
    pub fn path(&self) -> PathBuf {
        self.path.clone()
    }

    pub fn file_name(&self) -> OsString {
        match self.path.file_name() {
            Some(file_name) => file_name.to_os_string(),
            None => panic!("Could not get file name of DirEntry")
        }
    }

    pub fn metadata(&self) -> io::Result<FileAttr> {
        stat(&self.path)
    }

    pub fn file_type(&self) -> io::Result<FileType> {
        Ok(self.file_type)
    }
}

#[derive(Clone, Debug)]
pub struct OpenOptions {
    flags: u64,
    truncate: bool,
    create: bool,
    create_new: bool,
    append: bool,
}

const READ_MODE: u64 = 1;
const WRITE_MODE: u64 = 2;
const APPEND_MODE: u64 = 4;


impl OpenOptions {
    pub fn new() -> OpenOptions {
        OpenOptions {
            flags: 0,
            truncate: false,
            create: false,
            create_new: false,
            append: false,
        }
    }

    pub fn read(&mut self, read: bool) {
        if read {
            self.flags |= READ_MODE;
        } else {
            self.flags &= !READ_MODE;
        }
    }
    pub fn write(&mut self, write: bool) {
        if write {
            self.flags |= WRITE_MODE;
            self.flags |= APPEND_MODE;
        } else {
            self.flags &= !WRITE_MODE;
        }
    }
    pub fn append(&mut self, append: bool) {
        if append {
            self.write(true);
        }

        self.append = append;
    }
    pub fn truncate(&mut self, truncate: bool) {
        self.truncate = truncate;
    }

    pub fn create(&mut self, create: bool) {
        self.create = create;
    }

    pub fn create_new(&mut self, create_new: bool) {
        self.create_new = create_new;
    }
}

pub struct File {
    inner: FileHandle,
    pos: AtomicU64,
    append: bool,
    attr: FileAttr
}

unsafe impl Send for File {}

impl crate::ops::Drop for File {
    fn drop(&mut self) {
        if self.inner.handle == 0 {
            return;
        }
        unsafe {
            nnsdk::fs::CloseFile(
                self.inner
            );
        }
    }
}

fn cstr(path: &Path) -> io::Result<CString> {
    CString::new(
        path.to_str()
            .ok_or(io::Error::from(io::ErrorKind::InvalidInput))?
            .as_bytes()
    ).map_err(io::Error::from)
}

macro_rules! ret_if_null {
    ($expr:expr) => {
        if ($expr.handle) == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Cannot treat directory as file"
            ))
        }
    };
}

impl File {
    pub fn open(path: &Path, opts: &OpenOptions) -> io::Result<File> {
        if !path.is_absolute() {
            return Err(io::Error::new(io::ErrorKind::NotFound, "Path must be absolute"));
        }

        let path = cstr(path)?;

        let mut inner = FileHandle { handle: 0 as _ };
        
        let mut entry_type = 0u32;

        unsafe {
            match nnsdk::fs::GetEntryType(&mut entry_type, path.as_ptr() as _) {
                0 => {
                    if opts.create_new {
                        return Err(io::Error::new(io::ErrorKind::AlreadyExists, path.to_str().unwrap()));
                    }
                },
                // Path does not exist
                514 if opts.create => {
                    nnsdk::fs::CreateFile(path.as_ptr() as _, 0);
                    entry_type = NN_ENTRY_FILE;
                },
                result => return Err(io::Error::new(io::ErrorKind::Other, format!("Result code: {}", result))),
            };
        }
        
        let file_type = match entry_type {
            NN_ENTRY_DIR => FileType::Dir,
            NN_ENTRY_FILE => FileType::File,
            _ => panic!("Invalid entry type in directory")
        };


        if let FileType::Dir = file_type {
            return Ok(File {
                inner, pos: AtomicU64::new(0), append: false, attr: FileAttr { size: AtomicU64::new(0), file_type },
            })
        }

        unsafe { 
            r_try!(
                nnsdk::fs::OpenFile(
                    &mut inner,
                    path.as_ptr() as _,
                    opts.flags as _
                )
            )?;
        }

        if inner.handle == 0 {
            Err(io::Error::new(io::ErrorKind::NotFound, "Returned file handle was null"))
        } else {
            let mut filesize = 0;
            
            unsafe {
                r_try!(nnsdk::fs::GetFileSize(&mut filesize, inner))?;
            }

            let size = AtomicU64::new(filesize as _);

            let pos = if opts.append {
                AtomicU64::new(filesize as _)
            } else {
                AtomicU64::new(0)
            };

            let file = File {
                inner,
                pos,
                append: opts.flags & APPEND_MODE != 0,
                attr: FileAttr { size, file_type },
            };

            if opts.truncate {
                file.truncate(0)?;
            }

            Ok(file)
        }
    }

    pub fn file_attr(&self) -> io::Result<FileAttr> {
        Ok(self.attr.clone())
    }

    pub fn fsync(&self) -> io::Result<()> {
        ret_if_null!(self.inner);
        unsafe { r_try!(nnsdk::fs::FlushFile(self.inner)) }
    }

    pub fn datasync(&self) -> io::Result<()> {
        self.fsync()
    }

    pub fn lock(&self) -> io::Result<()> {
        unsupported()
    }

    pub fn lock_shared(&self) -> io::Result<()> {
        unsupported()
    }

    pub fn try_lock(&self) -> Result<(), TryLockError> {
        Err(TryLockError::Error(unsupported_err()))
    }

    pub fn try_lock_shared(&self) -> Result<(), TryLockError> {
        Err(TryLockError::Error(unsupported_err()))
    }

    pub fn unlock(&self) -> io::Result<()> {
        unsupported()
    }

    pub fn truncate(&self, size: u64) -> io::Result<()> {
        ret_if_null!(self.inner);
        let rc = unsafe {
            nnsdk::fs::SetFileSize(self.inner, size as _)
        };

        self.attr.set_size(size);

        r_try!(rc)
    }

    pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        let mut out_size = 0;

        cvt(unsafe {
            nnsdk::fs::ReadFile1(
                &mut out_size,
                self.inner,
                self.pos() as _,
                buf.as_ptr() as _,
                buf.len() as _
            )
        } as i32)?;

        // Needed to keep track of the position in the file
        self.pos.fetch_add(out_size as u64, Ordering::SeqCst);

        Ok(out_size as usize)
    }

    pub fn read_vectored(&self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        io::default_read_vectored(|b| self.read(b), bufs)
    }

    #[inline]
    pub fn is_read_vectored(&self) -> bool {
        false
    }

    pub fn read_buf(&self, mut cursor: BorrowedCursor<'_>) -> io::Result<()> {
        io::default_read_buf(|b| self.read(b), cursor)
    }

    pub fn write(&self, buf: &[u8]) -> io::Result<usize> {
        ret_if_null!(self.inner);

        if !self.append && self.pos.load(Ordering::SeqCst) + buf.len() as u64 > self.attr.size.load(Ordering::SeqCst) {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, format!("The length of the buffer ({}) is larger than the size of the file ({}). Consider using the Append flag.", buf.len(), self.pos.load(Ordering::SeqCst))));
        }

        let rc = unsafe {
            nnsdk::fs::WriteFile(
                self.inner,
                self.pos() as _,
                buf.as_ptr() as _,
                buf.len() as u64,
                &nnsdk::fs::WriteOption { flags: 1 }
            )
        };

        if rc == 0 {
            self.pos.fetch_add(buf.len() as u64, Ordering::SeqCst);
            if self.pos() > self.attr.size() {
                self.attr.set_size(self.pos());
            }
            Ok(buf.len())
        } else {
            Err(io::Error::from_raw_os_error(rc as _))
        }
    }

    pub fn write_vectored(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        io::default_write_vectored(|b| self.write(b), bufs)
    }

    #[inline]
    pub fn is_write_vectored(&self) -> bool {
        false
    }

    pub fn flush(&self) -> io::Result<()> {
        self.fsync()
    }

    fn pos(&self) -> u64 {
        self.pos.load(Ordering::SeqCst)
    }

    pub fn seek(&self, pos: SeekFrom) -> io::Result<u64> {
        match pos {
            SeekFrom::Start(offset) => {
                self.pos.store(offset, Ordering::SeqCst);
            },
            SeekFrom::Current(offset) => {
                let pos = (self.pos.load(Ordering::SeqCst) as i64) + offset;
                if pos < 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Attempted to seek to an invalid or negative offset"
                    ))
                }
                self.pos.store(pos as u64, Ordering::SeqCst);
            },
            SeekFrom::End(offset) => {
                if offset > 0 || (-offset as u64) > self.attr.size() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Attempted to seek to an invalid or negative offset"
                    ))
                }
                if offset < 0 {
                    self.pos.store(self.attr.size() - (-offset as u64), Ordering::SeqCst);
                } else {
                    self.pos.store(self.attr.size() + (offset as u64), Ordering::SeqCst);
                }
            },
        };

        Ok(self.pos.load(Ordering::SeqCst))
    }

    pub fn tell(&self) -> io::Result<u64> {
        self.seek(SeekFrom::Current(0))
    }

    pub fn size(&self) -> Option<io::Result<u64>> {
        Some(Ok(self.attr.size()))
    }

    pub fn duplicate(&self) -> io::Result<File> {
        // This feels super wrong and will probably break something
        Ok(File {
            inner: self.inner.clone(),
            pos: AtomicU64::new(self.pos()),
            append: self.append.clone(),
            attr: self.attr.clone()
        })
    }

    pub fn set_permissions(&self, _perm: FilePermissions) -> io::Result<()> {
        Ok(())
    }

    pub fn diverge(&self) -> ! {
        panic!("file diverge")
    }

    pub fn set_times(&self, times: FileTimes) -> io::Result<()> {
        unimplemented!()
    }
}

impl DirBuilder {
    pub fn new() -> DirBuilder {
        DirBuilder {}
    }

    pub fn mkdir(&self, path: &Path) -> io::Result<()> {
        let path = cstr(path)?;

        unsafe {
            r_try!(nnsdk::fs::CreateDirectory(path.as_ptr() as *const _))
        }
    }
}

impl fmt::Debug for File {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[Open File]")
    }
}

pub fn readdir(path: &Path) -> io::Result<ReadDir> {
    let path_buf = path.to_path_buf();
    let path = cstr(path)?;

    let mut dir_handle = nnsdk::fs::DirectoryHandle { handle: 0 };
    let mut count: i64 = 0;

    unsafe {
        r_try!(
            nnsdk::fs::OpenDirectory(
                &mut dir_handle as _,
                path.as_ptr() as _,
                nnsdk::fs::OpenDirectoryMode_OpenDirectoryMode_All as _
            )
        )?;

        r_try!(
            nnsdk::fs::GetDirectoryEntryCount(
                &mut count as _,
                dir_handle
            )
        )?;
    }

    let mut entries: Vec<NinDirEntry> = Vec::with_capacity(count as usize);

    unsafe {
        entries.set_len(count as usize);

        r_try!(
            nnsdk::fs::ReadDirectory(
                &mut count,
                entries.as_mut_ptr(),
                dir_handle,
                count
            )
        )?;

        nnsdk::fs::CloseDirectory(dir_handle);
    }

    Ok(ReadDir {
        inner_iter: entries.into_iter(),
        root: path_buf
    })
}

pub fn unlink(_p: &Path) -> io::Result<()> {
    let path = cstr(_p)?;
    unsafe {
        r_try!(nnsdk::fs::DeleteFile(path.as_ptr() as _))
    }
}

pub fn rename(old: &Path, new: &Path) -> io::Result<()> {
    let stat = stat(old)?;
    let old = cstr(old)?;
    let new = cstr(new)?;

    r_try!(if let FileType::File = stat.file_type() {
        unsafe {
            nnsdk::fs::RenameFile(old.as_ptr() as _, new.as_ptr() as _)
        }
    } else {
        unsafe {
            nnsdk::fs::RenameDirectory(old.as_ptr() as _, new.as_ptr() as _)
        }
    })
}

pub fn set_times(_p: &Path, _times: FileTimes) -> io::Result<()> {
    unsupported()
}

pub fn set_times_nofollow(_p: &Path, _times: FileTimes) -> io::Result<()> {
    unsupported()
}

pub fn set_perm(_p: &Path, _perm: FilePermissions) -> io::Result<()> {
    Ok(())
}

pub fn rmdir(path: &Path) -> io::Result<()> {
    let path = cstr(path)?;

    if (nnsdk::fs::DeleteDirectory as *const ()).is_null() {
        panic!("DeleteDirectory is null");
    }

    unsafe {
        r_try!(nnsdk::fs::DeleteDirectory(path.as_ptr() as _))
    }
}

pub fn remove_dir_all(path: &Path) -> io::Result<()> {
    let path = cstr(path)?;

    if (nnsdk::fs::DeleteDirectoryRecursively as *const ()).is_null() {
        panic!("DeleteDirectoryRecursively is null");
    }

    unsafe {
        r_try!(nnsdk::fs::DeleteDirectoryRecursively(path.as_ptr() as _))
    }
}

pub fn readlink(_p: &Path) -> io::Result<PathBuf> {
    unsupported()
}

pub fn symlink(_src: &Path, _dst: &Path) -> io::Result<()> {
    unsupported()
}

pub fn link(_src: &Path, _dst: &Path) -> io::Result<()> {
    unsupported()
}

fn get_entry_type(cstr: &CStr) -> io::Result<FileType> {
    let mut entry_type: u32 = 0;
    unsafe {
        r_try!(
            nnsdk::fs::GetEntryType(
                &mut entry_type,
                cstr.as_ptr() as _
            )
        )?;
    }

    match entry_type {
        NN_ENTRY_DIR => Ok(FileType::Dir),
        NN_ENTRY_FILE => Ok(FileType::File),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Could not stat directory type"
        ))
    }
}

pub fn stat(path: &Path) -> io::Result<FileAttr> {
    if !path.is_absolute() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "Path must be absolute"));
    }

    let path = cstr(path)?;
    
    let file_type = get_entry_type(&path)?;

    let size: Result<AtomicU64, io::Error> = match file_type {
        FileType::File => {
            let mut inner = FileHandle { handle: 0 as _ };

            unsafe { 
                r_try!(
                    nnsdk::fs::OpenFile(
                        &mut inner,
                        path.as_ptr() as _,
                        READ_MODE as _
                    )
                )?;

                let mut filesize = 0;
                
                if inner.handle == 0 {
                    return Err(io::Error::new(io::ErrorKind::NotFound, "Returned file handle was null"));
                } else {
                    r_try!(nnsdk::fs::GetFileSize(&mut filesize, inner))?;
                }
                
                nnsdk::fs::CloseFile(inner);

                Ok(AtomicU64::new(filesize as _))
            }
        },
        FileType::Dir => Ok(AtomicU64::new(0)),
    };

    Ok(FileAttr { size: size?, file_type })
}

pub fn lstat(path: &Path) -> io::Result<FileAttr> {
    stat(path)
}

pub fn canonicalize(_p: &Path) -> io::Result<PathBuf> {
    unsupported()
}

pub fn copy(from: &Path, to: &Path) -> io::Result<u64> {
    crate::io::copy(
        &mut crate::fs::File::open(from)?,
        &mut crate::fs::File::create(to)?,
    )
}
