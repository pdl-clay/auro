// SPDX-License-Identifier: MIT OR Apache-2.0

//! Fail-closed validation of the update journal's local trust boundary.
//!
//! The journal names its own verification key, staged artifact, platform target and relaunch
//! command. A journal planted by another local user is therefore equivalent to that user choosing
//! which signed artifact and executable the helper runs. [`ApplyJournal`](crate::ApplyJournal)
//! accepts a journal only from a directory and file owned by the current user that no other user
//! can write.
//!
//! The checks operate on opened handles, not on strings. Unix refuses symlinks, checks the file
//! and directory owner/mode, and walks the ancestors so a shared parent cannot replace the
//! directory. Windows opens reparse points themselves, rejects them, and inspects the owner and
//! DACL. Neither path defends against root/Administrator, a compromised same-user session, or a
//! hostile kernel; spec 0033 records the threat model and the remaining TOCTOU/ACL limits.

use std::fs::File;
use std::path::Path;

use crate::UpdateError;

/// Opens and validates a journal file for reading.
pub(crate) fn open_journal(path: &Path) -> Result<File, UpdateError> {
    validate_parent(path)?;
    let file = platform::open_file(path, true, false, false)?;
    platform::validate_file(&file, path)?;
    Ok(file)
}

/// Validates the journal's parent directory and its ancestors.
pub(crate) fn validate_parent(path: &Path) -> Result<(), UpdateError> {
    platform::validate_parent(path)
}

/// Opens (and optionally creates) the restart lock, validating it before it is locked.
pub(crate) fn open_lock(path: &Path, create: bool) -> Result<File, UpdateError> {
    validate_parent(path)?;
    let file = platform::open_file(path, true, true, create)?;
    platform::validate_file(&file, path)?;
    Ok(file)
}

/// Validates an existing lock file; a missing lock is allowed because the helper creates it.
pub(crate) fn validate_lock_path(path: &Path) -> Result<(), UpdateError> {
    validate_parent(path)?;
    match platform::open_file(path, true, true, false) {
        Ok(file) => platform::validate_file(&file, path),
        Err(UpdateError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub(crate) fn trust_error(path: &Path, reason: &'static str) -> UpdateError {
    UpdateError::JournalTrust {
        path: path.to_path_buf(),
        reason,
    }
}

#[cfg(unix)]
mod platform {
    use std::fs::{File, Metadata, OpenOptions};
    use std::io;
    use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
    use std::path::Path;

    use super::{UpdateError, trust_error};

    pub(super) fn open_file(
        path: &Path,
        read: bool,
        write: bool,
        create: bool,
    ) -> Result<File, UpdateError> {
        let mut options = OpenOptions::new();
        options
            .read(read)
            .write(write)
            .create(create)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW);
        options
            .open(path)
            .map_err(|error| map_open_error(path, error))
    }

    pub(super) fn validate_file(file: &File, path: &Path) -> Result<(), UpdateError> {
        let metadata = file.metadata().map_err(UpdateError::Io)?;
        if !metadata.is_file() {
            return Err(trust_error(path, "not a regular file"));
        }
        validate_owner_and_mode(path, &metadata, false, false)
    }

    pub(super) fn validate_parent(path: &Path) -> Result<(), UpdateError> {
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        #[cfg(test)]
        eprintln!("trust: validate_parent path={:?} parent={:?}", path, parent);
        let directory = open_directory(parent)?;
        let metadata = directory.metadata().map_err(UpdateError::Io)?;
        if !metadata.is_dir() {
            return Err(trust_error(parent, "journal parent is not a directory"));
        }
        validate_owner_and_mode(parent, &metadata, true, true)?;
        validate_ancestors(parent)
    }

    fn open_directory(path: &Path) -> Result<File, UpdateError> {
        let mut options = OpenOptions::new();
        options
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY);
        options.open(path).map_err(|error| {
            #[cfg(test)]
            eprintln!("trust: open_directory({:?}) failed: {error}", path);
            map_open_error(path, error)
        })
    }

    fn validate_ancestors(parent: &Path) -> Result<(), UpdateError> {
        let mut current = parent;
        while let Some(ancestor) = current.parent() {
            if ancestor.as_os_str().is_empty() {
                break;
            }
            let directory = open_directory(ancestor)?;
            let metadata = directory.metadata().map_err(UpdateError::Io)?;
            if !metadata.is_dir() {
                return Err(trust_error(ancestor, "journal ancestor is not a directory"));
            }
            validate_owner_and_mode(ancestor, &metadata, true, false)?;
            if ancestor == current {
                break;
            }
            current = ancestor;
        }
        Ok(())
    }

    // libc::S_ISVTX is u16 on macOS and u32 on Linux; the cast is required on macOS and a no-op
    // on Linux, where clippy would otherwise call it unnecessary.
    #[allow(clippy::unnecessary_cast)]
    fn validate_owner_and_mode(
        path: &Path,
        metadata: &Metadata,
        directory: bool,
        immediate: bool,
    ) -> Result<(), UpdateError> {
        // SAFETY: geteuid has no preconditions.
        let euid = unsafe { libc::geteuid() };
        let ancestor = directory && !immediate;
        // A root-owned ancestor with no write bit for other users cannot replace the journal.
        let owned = metadata.uid() == euid || (ancestor && metadata.uid() == 0);
        if !owned {
            return Err(trust_error(
                path,
                "journal path is not owned by the current user",
            ));
        }
        let mode = metadata.mode();
        if mode & 0o022 != 0 {
            // A sticky world-writable ancestor (for example /tmp) cannot remove or rename an
            // entry owned by this user, so it is allowed for ancestors. The immediate journal
            // directory must not be writable by another principal at all.
            let sticky_ancestor = directory
                && !immediate
                && mode & (libc::S_ISVTX as u32) != 0
                && (metadata.uid() == 0 || metadata.uid() == euid);
            if !sticky_ancestor {
                return Err(trust_error(
                    path,
                    "journal path is writable by another user or group",
                ));
            }
        }
        Ok(())
    }

    fn map_open_error(path: &Path, error: io::Error) -> UpdateError {
        if error.raw_os_error() == Some(libc::ELOOP) {
            trust_error(path, "journal path is a symbolic link")
        } else {
            UpdateError::Io(error)
        }
    }
}

#[cfg(windows)]
mod platform {
    use std::ffi::c_void;
    use std::fs::{File, OpenOptions};
    use std::mem::size_of;
    use std::os::windows::fs::OpenOptionsExt as _;
    use std::os::windows::io::AsRawHandle as _;
    use std::path::Path;
    use std::ptr::null_mut;

    use windows::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE, HLOCAL, LocalFree};
    use windows::Win32::Security::Authorization::{GetSecurityInfo, SE_FILE_OBJECT};
    use windows::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACL, ACL_SIZE_INFORMATION, AclSizeInformation, CreateWellKnownSid,
        DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetAclInformation, GetLengthSid,
        GetTokenInformation, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, TOKEN_QUERY,
        TOKEN_USER, TokenUser, WinBuiltinAdministratorsSid, WinCreatorOwnerRightsSid,
        WinCreatorOwnerSid, WinLocalSystemSid,
    };
    use windows::Win32::Storage::FileSystem::{
        DELETE, FILE_ADD_FILE, FILE_ADD_SUBDIRECTORY, FILE_APPEND_DATA,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO, FILE_DELETE_CHILD,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_WRITE_ATTRIBUTES,
        FILE_WRITE_DATA, FILE_WRITE_EA, FileAttributeTagInfo, GetFileInformationByHandleEx,
        WRITE_DAC, WRITE_OWNER,
    };
    use windows::Win32::System::SystemServices::{
        ACCESS_ALLOWED_ACE_TYPE, ACCESS_ALLOWED_OBJECT_ACE_TYPE, ACCESS_DENIED_ACE_TYPE,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    use super::{UpdateError, trust_error};

    const WRITE_ACCESS: u32 = FILE_WRITE_DATA.0
        | FILE_APPEND_DATA.0
        | FILE_WRITE_EA.0
        | FILE_WRITE_ATTRIBUTES.0
        | FILE_ADD_FILE.0
        | FILE_ADD_SUBDIRECTORY.0
        | FILE_DELETE_CHILD.0
        | DELETE.0
        | WRITE_DAC.0
        | WRITE_OWNER.0
        | 0x4000_0000 // GENERIC_WRITE
        | 0x1000_0000; // GENERIC_ALL

    pub(super) fn open_file(
        path: &Path,
        read: bool,
        write: bool,
        create: bool,
    ) -> Result<File, UpdateError> {
        let mut options = OpenOptions::new();
        options
            .read(read)
            .write(write)
            .create(create)
            .truncate(false)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0);
        options.open(path).map_err(UpdateError::Io)
    }

    pub(super) fn validate_file(file: &File, path: &Path) -> Result<(), UpdateError> {
        let handle = HANDLE(file.as_raw_handle());
        reject_reparse(handle, path)?;
        validate_owner_and_dacl(handle, path)
    }

    pub(super) fn validate_parent(path: &Path) -> Result<(), UpdateError> {
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let mut options = OpenOptions::new();
        options
            .read(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0);
        let directory = options.open(parent).map_err(UpdateError::Io)?;
        let handle = HANDLE(directory.as_raw_handle());
        reject_reparse(handle, parent)?;
        validate_owner_and_dacl(handle, parent)
    }

    fn reject_reparse(handle: HANDLE, path: &Path) -> Result<(), UpdateError> {
        let mut info = FILE_ATTRIBUTE_TAG_INFO::default();
        // SAFETY: the handle belongs to a live File and the buffer matches its documented type.
        unsafe {
            GetFileInformationByHandleEx(
                handle,
                FileAttributeTagInfo,
                &mut info as *mut _ as *mut c_void,
                size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
            )
        }
        .map_err(|_| trust_error(path, "cannot read file attributes"))?;
        if info.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
            return Err(trust_error(path, "journal path is a reparse point"));
        }
        Ok(())
    }

    fn validate_owner_and_dacl(handle: HANDLE, path: &Path) -> Result<(), UpdateError> {
        let user = current_user_sid()?;
        let system = well_known_sid(WinLocalSystemSid)?;
        let administrators = well_known_sid(WinBuiltinAdministratorsSid)?;
        let creator_owner = well_known_sid(WinCreatorOwnerSid)?;
        let creator_owner_rights = well_known_sid(WinCreatorOwnerRightsSid)?;

        let mut owner = PSID(null_mut());
        let mut dacl: *mut ACL = null_mut();
        let mut descriptor = PSECURITY_DESCRIPTOR(null_mut());
        // SAFETY: owner/dacl/descriptor are valid out-pointers and the handle is live.
        let result = unsafe {
            GetSecurityInfo(
                handle,
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                Some(&mut owner),
                None,
                Some(&mut dacl),
                None,
                Some(&mut descriptor),
            )
        };
        if result != ERROR_SUCCESS {
            return Err(trust_error(path, "cannot read file security information"));
        }
        // SAFETY: the descriptor owns the memory until LocalFree; the pointers above point into it.
        let result = (|| {
            if !sid_is_principal(owner, &user)
                && !sid_is_principal(owner, &system)
                && !sid_is_principal(owner, &administrators)
                && !sid_is_principal(owner, &creator_owner)
                && !sid_is_principal(owner, &creator_owner_rights)
            {
                return Err(trust_error(
                    path,
                    "journal path is not owned by the current user",
                ));
            }
            if dacl.is_null() {
                return Err(trust_error(path, "journal path has no DACL"));
            }
            validate_dacl(
                dacl,
                path,
                &user,
                &system,
                &administrators,
                &creator_owner,
                &creator_owner_rights,
            )
        })();
        // SAFETY: GetSecurityInfo allocated the descriptor; the owner/dacl pointers are borrowed.
        unsafe {
            LocalFree(Some(HLOCAL(descriptor.0)));
        }
        result
    }

    fn validate_dacl(
        dacl: *mut ACL,
        path: &Path,
        user: &[u8],
        system: &[u8],
        administrators: &[u8],
        creator_owner: &[u8],
        creator_owner_rights: &[u8],
    ) -> Result<(), UpdateError> {
        let mut info = ACL_SIZE_INFORMATION::default();
        // SAFETY: dacl is a valid ACL returned by GetSecurityInfo and info matches the class.
        unsafe {
            GetAclInformation(
                dacl,
                &mut info as *mut _ as *mut c_void,
                size_of::<ACL_SIZE_INFORMATION>() as u32,
                AclSizeInformation,
            )
        }
        .map_err(|_| trust_error(path, "cannot enumerate journal DACL"))?;

        for index in 0..info.AceCount {
            let mut ace: *mut c_void = null_mut();
            // SAFETY: index is within AceCount and ace receives a pointer into the ACL.
            unsafe { GetAce(dacl, index, &mut ace) }
                .map_err(|_| trust_error(path, "cannot read journal DACL entry"))?;
            if ace.is_null() {
                return Err(trust_error(path, "journal DACL entry is null"));
            }
            // SAFETY: GetAce returned an ACE whose first field is ACE_HEADER.
            let header = unsafe { &*(ace as *const windows::Win32::Security::ACE_HEADER) };
            match u32::from(header.AceType) {
                ACCESS_ALLOWED_ACE_TYPE => {
                    // SAFETY: an allowed ACE has the ACCESS_ALLOWED_ACE layout.
                    let allowed = unsafe { &*(ace as *const ACCESS_ALLOWED_ACE) };
                    if allowed.Mask & WRITE_ACCESS == 0 {
                        continue;
                    }
                    // SAFETY: SidStart is the first bytes of the variable-length SID.
                    let sid = PSID(&allowed.SidStart as *const u32 as *mut c_void);
                    if !sid_is_principal(sid, user)
                        && !sid_is_principal(sid, system)
                        && !sid_is_principal(sid, administrators)
                        && !sid_is_principal(sid, creator_owner)
                        && !sid_is_principal(sid, creator_owner_rights)
                    {
                        return Err(trust_error(
                            path,
                            "journal DACL grants write access to another principal",
                        ));
                    }
                }
                ACCESS_DENIED_ACE_TYPE => {}
                ACCESS_ALLOWED_OBJECT_ACE_TYPE => {
                    return Err(trust_error(
                        path,
                        "journal DACL contains an unsupported object ACE",
                    ));
                }
                _ => {
                    return Err(trust_error(
                        path,
                        "journal DACL contains an unsupported ACE type",
                    ));
                }
            }
        }
        Ok(())
    }

    fn current_user_sid() -> Result<Vec<u8>, UpdateError> {
        let mut token = HANDLE(null_mut());
        // SAFETY: GetCurrentProcess returns a pseudo-handle; token receives a real handle.
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }
            .map_err(|_| trust_error(Path::new("."), "cannot open process token"))?;
        let mut length = 0u32;
        // SAFETY: a null buffer with length zero asks for the required size.
        let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut length) };
        if length == 0 {
            // SAFETY: token was opened by this function and is closed below.
            unsafe {
                let _ = CloseHandle(token);
            }
            return Err(trust_error(
                Path::new("."),
                "cannot size the process token user",
            ));
        }
        let mut buffer = vec![0u8; length as usize];
        // SAFETY: buffer is at least `length` bytes and token is live.
        let result = unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                Some(buffer.as_mut_ptr() as *mut c_void),
                length,
                &mut length,
            )
        };
        // SAFETY: token was opened by this function.
        unsafe {
            let _ = CloseHandle(token);
        }
        result.map_err(|_| trust_error(Path::new("."), "cannot read the process token user"))?;
        // SAFETY: the buffer contains a TOKEN_USER followed by its SID.
        let token_user = unsafe { &*(buffer.as_ptr() as *const TOKEN_USER) };
        let sid = token_user.User.Sid;
        // SAFETY: sid points into the live token buffer and GetLengthSid returns its byte length.
        let length = unsafe { GetLengthSid(sid) } as usize;
        if length == 0 {
            return Err(trust_error(
                Path::new("."),
                "the process token user has no SID",
            ));
        }
        // SAFETY: sid is valid for `length` bytes.
        Ok(unsafe { std::slice::from_raw_parts(sid.0 as *const u8, length) }.to_vec())
    }

    fn well_known_sid(
        kind: windows::Win32::Security::WELL_KNOWN_SID_TYPE,
    ) -> Result<Vec<u8>, UpdateError> {
        let mut buffer = vec![0u8; windows::Win32::Security::SECURITY_MAX_SID_SIZE as usize];
        let mut length = buffer.len() as u32;
        // SAFETY: buffer is SECURITY_MAX_SID_SIZE bytes and length matches.
        unsafe {
            CreateWellKnownSid(
                kind,
                None,
                Some(PSID(buffer.as_mut_ptr() as *mut c_void)),
                &mut length,
            )
        }
        .map_err(|_| trust_error(Path::new("."), "cannot create a well-known SID"))?;
        buffer.truncate(length as usize);
        Ok(buffer)
    }

    fn sid_is_principal(sid: PSID, expected: &[u8]) -> bool {
        if sid.0.is_null() || expected.is_empty() {
            return false;
        }
        // SAFETY: both SIDs are valid and expected is a byte slice holding a SID.
        unsafe { EqualSid(sid, PSID(expected.as_ptr() as *mut c_void)).is_ok() }
    }
}

#[cfg(not(any(unix, windows)))]
compile_error!("update journal trust validation needs a Unix or Windows implementation");
