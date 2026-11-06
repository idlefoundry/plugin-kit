//! Makes Idle Foundry's offline signing key (decisions.md R-SKIN-KEYS):
//! `cargo run -p skin-pack --example pack_keygen -- <secret-key-file> [idlefoundry-1]`.
//! The destination must be new, outside every Git work tree, and in an existing folder.

use std::io::Write;
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

fn main() {
    if let Err(why) = run() {
        eprintln!("pack_keygen: {why}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (path, id) = match args.as_slice() {
        [path] if !path.starts_with('-') => (path, "idlefoundry-1"),
        [path, id] if !path.starts_with('-') => (path, id.as_str()),
        _ => return Err("pack_keygen <secret-key-file> [idlefoundry-1]".into()),
    };
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return Err("the key id uses letters, digits and hyphens only".into());
    }
    let path = destination(Path::new(path))?;
    let mut secret = Zeroizing::new([0u8; 32]);
    getrandom::fill(secret.as_mut()).map_err(|e| format!("system randomness: {e}"))?;
    let hex = Zeroizing::new(
        secret
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
    );
    let key = plugin_kit_skin_pack::signature::signing_key(&hex)
        .ok_or("cannot read the generated key")?;
    let mut file = create_private(&path).map_err(|e| format!("cannot create the secret: {e}"))?;
    file.write_all(hex.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|e| format!("cannot write the secret: {e}"))?;
    println!("Secret written to {} (owner-only access).", path.display());
    println!("Keep an offline backup. Never commit this secret or put it in a repository.");
    println!("Key id: {id}");
    println!(
        "Public key: {}",
        plugin_kit_skin_pack::signature::public_hex(&key)
    );
    println!("Paste this public constant into crates/skin-pack/src/keys.rs:");
    println!(
        "pub const IDLE_FOUNDRY_KEY: Option<TrustedKey> = Some(TrustedKey {{ id: {id:?}, public: {:?}, first_party: true }});",
        key.verifying_key().to_bytes()
    );
    Ok(())
}

/// Check both the spelling given and its resolved parent: a link cannot hide a work tree.
fn destination(path: &Path) -> Result<PathBuf, String> {
    let absolute = std::path::absolute(path).map_err(|e| e.to_string())?;
    let parent = absolute.parent().ok_or("a destination file is required")?;
    let resolved = parent
        .canonicalize()
        .map_err(|e| format!("the parent folder: {e}"))?;
    for root in [parent, resolved.as_path()] {
        for ancestor in root.ancestors() {
            if ancestor.join(".git").symlink_metadata().is_ok()
                || (ancestor.join("HEAD").is_file() && ancestor.join("objects").is_dir())
            {
                return Err("the secret must be OUTSIDE any Git work tree or repository".into());
            }
        }
    }
    // Also honour Git's work-tree configuration, without the caller's Git overrides.
    let mut git = std::process::Command::new("git");
    git.args(["rev-parse", "--is-inside-work-tree", "--is-bare-repository"])
        .current_dir(&resolved)
        .env("LC_ALL", "C");
    for (name, _) in
        std::env::vars_os().filter(|(name, _)| name.to_string_lossy().starts_with("GIT_"))
    {
        git.env_remove(name);
    }
    let result = git
        .output()
        .map_err(|e| format!("Git is required to check the destination: {e}"))?;
    if result.status.success() {
        if result
            .stdout
            .split(|b| b.is_ascii_whitespace())
            .any(|s| s == b"true")
        {
            return Err("the secret must be OUTSIDE any Git work tree or repository".into());
        }
    } else if !String::from_utf8_lossy(&result.stderr).contains("not a git repository") {
        return Err("Git could not establish that the destination is outside a repository".into());
    }
    let name = absolute
        .file_name()
        .ok_or("a destination file is required")?;
    #[cfg(windows)]
    if name.to_string_lossy().contains(':') {
        return Err("alternate data streams cannot hold the secret".into());
    }
    let path = resolved.join(name);
    if path.symlink_metadata().is_ok() {
        return Err("the secret's destination must not exist (links are refused too)".into());
    }
    Ok(path)
}

#[cfg(unix)]
fn create_private(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    if file.metadata()?.permissions().mode() & 0o777 != 0o600 {
        return Err(std::io::Error::other(
            "the destination did not provide owner-only read/write access; no secret was written",
        ));
    }
    Ok(file)
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn create_private(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::windows::{ffi::OsStrExt, io::FromRawHandle};
    use windows_sys::Win32::{
        Foundation::{INVALID_HANDLE_VALUE, LocalFree},
        Security::{
            Authorization::{
                ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
            },
            SECURITY_ATTRIBUTES,
        },
        Storage::FileSystem::{
            CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_WRITE,
            GetVolumeInformationByHandleW,
        },
        System::SystemServices::FILE_PERSISTENT_ACLS,
    };
    // A protected DACL: full access for the owner only, no inherited grants. Applied at
    // creation, before any secret is written (Microsoft's File Security and Access Rights).
    let sid = current_user_sid()?;
    let sddl: Vec<u16> = format!("O:{sid}D:P(A;;FA;;;{sid})\0")
        .encode_utf16()
        .collect();
    let mut descriptor = std::ptr::null_mut();
    // SAFETY: the SDDL is NUL-terminated; the descriptor's out pointer is valid.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error());
    }
    let security = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: the path and security descriptor live through the call. CREATE_NEW never
    // follows or overwrites an existing file. The returned handle is owned by this function.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_GENERIC_WRITE,
            0,
            &security,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    let error = std::io::Error::last_os_error();
    // SAFETY: the descriptor was allocated by the conversion above, and is no longer used.
    unsafe { LocalFree(descriptor) };
    if handle == INVALID_HANDLE_VALUE {
        return Err(error);
    }
    // SAFETY: a valid owned file handle, transferred exactly once to File.
    let file = unsafe { std::fs::File::from_raw_handle(handle) };
    let mut flags = 0;
    // SAFETY: the owned handle is live, flags is writable; all unused outputs are null.
    if unsafe {
        GetVolumeInformationByHandleW(
            handle,
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut flags,
            std::ptr::null_mut(),
            0,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error());
    }
    if flags & FILE_PERSISTENT_ACLS == 0 {
        return Err(std::io::Error::other(
            "the destination volume cannot enforce owner-only access; no secret was written",
        ));
    }
    Ok(file)
}

/// Use the actual user, including when elevated: the token's default owner can be a group.
#[cfg(windows)]
#[allow(unsafe_code)]
fn current_user_sid() -> std::io::Result<String> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, LocalFree},
        Security::{
            Authorization::ConvertSidToStringSidW, GetTokenInformation, TOKEN_QUERY, TOKEN_USER,
            TokenUser,
        },
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };
    let mut token = std::ptr::null_mut();
    // SAFETY: GetCurrentProcess returns a live pseudo-handle; token is a valid out pointer.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut needed = 0;
    // SAFETY: a size query, with no output buffer and a valid length pointer.
    unsafe { GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed) };
    // usize alignment suffices for TOKEN_USER and the SID that follows it.
    let mut buffer = vec![0usize; (needed as usize).div_ceil(std::mem::size_of::<usize>())];
    // SAFETY: the buffer is aligned and at least the requested size; token is still live.
    let result = unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    };
    let error = std::io::Error::last_os_error();
    // SAFETY: OpenProcessToken returned this owned handle, closed exactly once.
    unsafe { CloseHandle(token) };
    if result == 0 {
        return Err(error);
    }
    // SAFETY: GetTokenInformation filled the aligned buffer with TOKEN_USER and its SID.
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    let mut sid = std::ptr::null_mut();
    // SAFETY: the SID lives in buffer; sid is a valid out pointer.
    if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut sid) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut len = 0;
    // SAFETY: conversion returned a NUL-terminated UTF-16 allocation, alive until LocalFree.
    let text = unsafe {
        while *sid.add(len) != 0 {
            len += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(sid, len))
    };
    // SAFETY: the string was allocated by ConvertSidToStringSidW and copied above.
    unsafe { LocalFree(sid.cast()) };
    Ok(text)
}

#[cfg(not(any(unix, windows)))]
fn create_private(_: &Path) -> std::io::Result<std::fs::File> {
    Err(std::io::Error::other(
        "owner-only creation is supported on macOS, Linux and Windows",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_refused_inside_normal_and_linked_work_trees() {
        let dir = tempfile::tempdir().expect("a folder");
        for marker in [true, false] {
            let repo = dir.path().join(if marker { "normal" } else { "linked" });
            std::fs::create_dir_all(repo.join("nested")).expect("a folder");
            if marker {
                std::fs::create_dir(repo.join(".git")).expect("a Git directory");
            } else {
                std::fs::write(repo.join(".git"), "gitdir: elsewhere").expect("a Git file");
            }
            let path = repo.join("nested/secret.key");
            assert!(destination(&path).is_err());
            assert!(!path.exists());
        }
        assert!(destination(&dir.path().join("secret.key")).is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn alternate_data_streams_are_refused() {
        let dir = tempfile::tempdir().expect("a folder");
        assert!(destination(&dir.path().join("existing:secret.key")).is_err());
    }

    /// A SID as a security descriptor's text writes it: its SDDL alias if it has one, else as
    /// given.
    #[cfg(windows)]
    #[allow(unsafe_code)]
    fn sddl_name(sid: &str) -> String {
        use windows_sys::Win32::{
            Foundation::LocalFree,
            Security::{
                Authorization::{
                    ConvertSecurityDescriptorToStringSecurityDescriptorW,
                    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
                },
                OWNER_SECURITY_INFORMATION,
            },
        };
        let sddl: Vec<u16> = format!("O:{sid}\0").encode_utf16().collect();
        let mut descriptor = std::ptr::null_mut();
        // SAFETY: the SDDL is NUL-terminated; the descriptor's out pointer is valid.
        let made = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                std::ptr::null_mut(),
            )
        };
        assert_ne!(made, 0, "{sid}");
        let mut text = std::ptr::null_mut();
        // SAFETY: the descriptor was made above; text is a valid out pointer.
        let written = unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                descriptor,
                SDDL_REVISION_1,
                OWNER_SECURITY_INFORMATION,
                &mut text,
                std::ptr::null_mut(),
            )
        };
        // SAFETY: the conversion allocated the descriptor, no longer used.
        unsafe { LocalFree(descriptor) };
        assert_ne!(written, 0, "{sid}");
        let mut len = 0;
        // SAFETY: a NUL-terminated UTF-16 allocation, copied before it is freed.
        let back = unsafe {
            while *text.add(len) != 0 {
                len += 1;
            }
            String::from_utf16_lossy(std::slice::from_raw_parts(text, len))
        };
        // SAFETY: the conversion allocated text, now copied.
        unsafe { LocalFree(text.cast()) };
        back.strip_prefix("O:").unwrap_or(&back).to_owned()
    }

    #[cfg(windows)]
    #[test]
    #[allow(unsafe_code)]
    fn a_new_windows_file_has_one_protected_grant_for_the_current_user() {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::{
            Foundation::LocalFree,
            Security::{
                Authorization::{
                    ConvertSecurityDescriptorToStringSecurityDescriptorW, GetSecurityInfo,
                    SDDL_REVISION_1, SE_FILE_OBJECT,
                },
                DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION,
            },
        };
        let dir = tempfile::tempdir().expect("a folder");
        let path = destination(&dir.path().join("secret.key")).expect("outside Git");
        let file = create_private(&path).expect("created");
        let mut descriptor = std::ptr::null_mut();
        let info = OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
        // SAFETY: file is live; unused pointers are null, descriptor is a valid out pointer.
        let result = unsafe {
            GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                info,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut descriptor,
            )
        };
        assert_eq!(result, 0);
        let mut text = std::ptr::null_mut();
        // SAFETY: GetSecurityInfo returned this descriptor; text is a valid out pointer.
        let result = unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                descriptor,
                SDDL_REVISION_1,
                info,
                &mut text,
                std::ptr::null_mut(),
            )
        };
        // SAFETY: GetSecurityInfo allocated descriptor, and conversion has finished with it.
        unsafe { LocalFree(descriptor) };
        assert_ne!(result, 0);
        let mut len = 0;
        // SAFETY: conversion returned a NUL-terminated UTF-16 allocation, copied before freeing.
        let actual = unsafe {
            while *text.add(len) != 0 {
                len += 1;
            }
            String::from_utf16_lossy(std::slice::from_raw_parts(text, len))
        };
        // SAFETY: conversion allocated text, now copied and no longer used.
        unsafe { LocalFree(text.cast()) };
        // (As the descriptor's text names the user: a well-known account by its SDDL alias, as
        // the built-in Administrator's `LA`, which a CI runner's account can be.)
        let sid = sddl_name(&current_user_sid().expect("the current user"));
        assert_eq!(
            sddl_name("S-1-5-32-544"),
            "BA",
            "aliases are as the descriptor writes them"
        );
        let dacl = actual
            .strip_prefix(&format!("O:{sid}D:"))
            .unwrap_or_else(|| panic!("the user's ownership: {actual}"));
        let (flags, ace) = dacl.split_once('(').expect("one grant");
        assert!(flags.contains('P'), "the DACL is not protected: {actual}");
        assert_eq!(ace, format!("A;;FA;;;{sid})"));
        assert!(
            create_private(&path).is_err(),
            "an existing file was replaced"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_link_into_a_work_tree_is_refused_and_existing_files_are_never_replaced() {
        let dir = tempfile::tempdir().expect("a folder");
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).expect("a folder");
        std::fs::create_dir(repo.join(".git")).expect("a Git directory");
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&repo, &link).expect("a link");
        assert!(destination(&link.join("secret.key")).is_err());
        let path = dir.path().join("secret.key");
        std::fs::write(&path, "existing").expect("written");
        assert!(destination(&path).is_err());
        assert!(create_private(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "existing");
    }

    #[cfg(unix)]
    #[test]
    fn a_new_file_is_owner_only_from_creation() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("a folder");
        let path = destination(&dir.path().join("secret.key")).expect("outside Git");
        let file = create_private(&path).expect("created");
        assert_eq!(
            file.metadata().expect("metadata").permissions().mode() & 0o777,
            0o600
        );
    }
}
