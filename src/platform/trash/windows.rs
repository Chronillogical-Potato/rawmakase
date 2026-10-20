//! Windows: the Recycle Bin through the shell's `IFileOperation`, deleting
//! with `FOFX_RECYCLEONDELETE` and no UI. Its progress sink refuses, before
//! anything happens, a delete the shell would not recycle (no Recycle Bin
//! on the volume, a file too big for it) and keeps the item the Recycle Bin
//! made, whose path is the receipt. A refused delete is
//! [`TrashError::Unsupported`]; any other failure, abort or a delete without
//! a recycled item is [`TrashError::Failed`]. Neither deletes the file.
use crate::catalog::file_ops::{TrashError, TrashReceipt};
use std::cell::RefCell;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt as _;
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{E_ABORT, RPC_E_CHANGED_MODE};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance, CoInitializeEx,
    CoTaskMemFree, CoUninitialize,
};
use windows::Win32::UI::Shell::{
    FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FOFX_RECYCLEONDELETE,
    FileOperation, IFileOperation, IFileOperationProgressSink, IFileOperationProgressSink_Impl,
    IShellItem, SHCreateItemFromParsingName, SIGDN_FILESYSPATH, TSF_DELETE_RECYCLE_IF_POSSIBLE,
};
use windows::core::{ComObject, HRESULT, HSTRING, PCWSTR, Ref, implement};

pub(super) fn trash(path: &Path) -> Result<TrashReceipt, TrashError> {
    let _com = Com::init()?;
    let seen = recycle(path).map_err(|e| TrashError::Failed(said(&e)))?;
    match seen {
        Seen { refused: true, .. } => Err(TrashError::Unsupported),
        Seen {
            failed: Some(e), ..
        } => Err(TrashError::Failed(e)),
        Seen { aborted: true, .. } => Err(TrashError::Failed("the delete was cancelled".into())),
        Seen {
            recycled: Some(location),
            ..
        } => Ok(TrashReceipt { location }),
        Seen { .. } => Err(TrashError::Failed(
            "the Recycle Bin didn't say where the file went".into(),
        )),
    }
}

/// What one delete did, as its sink and the operation saw it.
#[derive(Default)]
struct Seen {
    /// The sink refused it: it would not have been recycled.
    refused: bool,
    aborted: bool,
    failed: Option<String>,
    recycled: Option<PathBuf>,
}

/// Deletes `path` to the Recycle Bin. An error is a step that didn't
/// happen; what the shell did is in [`Seen`].
fn recycle(path: &Path) -> windows::core::Result<Seen> {
    let sink = ComObject::new(Sink::default());
    let progress: IFileOperationProgressSink = sink.to_interface();
    // SAFETY: COM is initialised on this thread (`Com`), every pointer the
    // calls take is a live interface or string borrowed for the call, and
    // the interfaces are released when dropped.
    let (performed, aborted) = unsafe {
        let operation: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL)?;
        operation.SetOperationFlags(
            FOFX_RECYCLEONDELETE | FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT,
        )?;
        let item: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(path), None)?;
        operation.DeleteItem(&item, &progress)?;
        let performed = operation.PerformOperations();
        (performed, operation.GetAnyOperationsAborted()?.as_bool())
    };
    let mut seen = sink.seen.take();
    seen.aborted = aborted;
    if let Err(e) = performed
        && !seen.refused
    {
        seen.failed.get_or_insert(said(&e));
    }
    Ok(seen)
}

/// COM on this thread, single-threaded as `IFileOperation` needs, for as
/// long as it lives.
struct Com;

impl Com {
    fn init() -> Result<Self, TrashError> {
        // SAFETY: no reserved pointer; a success is paired with the
        // `CoUninitialize` in `drop`, on this thread.
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
        if hr == RPC_E_CHANGED_MODE {
            return Err(TrashError::Failed(
                "this thread's COM is multithreaded, and the Recycle Bin needs it not to be".into(),
            ));
        }
        hr.ok()
            .map_err(|e| TrashError::Failed(said(&e)))
            .map(|()| Self)
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        // SAFETY: `init` succeeded on this thread (S_OK or S_FALSE), which
        // asks for one call each.
        unsafe { CoUninitialize() }
    }
}

/// Watches the delete: refuses one the shell wouldn't recycle, and keeps
/// what it did.
#[implement(IFileOperationProgressSink)]
#[derive(Default)]
struct Sink {
    seen: RefCell<Seen>,
}

impl IFileOperationProgressSink_Impl for Sink_Impl {
    fn PreDeleteItem(&self, flags: u32, _item: Ref<IShellItem>) -> windows::core::Result<()> {
        if flags & TSF_DELETE_RECYCLE_IF_POSSIBLE.0 as u32 == 0 {
            self.seen.borrow_mut().refused = true;
            return Err(E_ABORT.into());
        }
        Ok(())
    }

    fn PostDeleteItem(
        &self,
        _flags: u32,
        _item: Ref<IShellItem>,
        result: HRESULT,
        recycled: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        // Any success, not only S_OK: the shell reports a recycled delete
        // with codes of its own; what makes it one is the item it made.
        // That is asked of the shell before `seen` is borrowed, so nothing
        // it calls back can find it borrowed.
        let found = result.is_ok().then(|| recycled.as_ref().map(file_path));
        let mut seen = self.seen.borrow_mut();
        let Some(found) = found else {
            seen.failed = Some(said(&windows::core::Error::from_hresult(result)));
            return Ok(());
        };
        match found {
            Some(Ok(path)) => seen.recycled = Some(path),
            Some(Err(e)) => seen.failed = Some(said(&e)),
            None => {
                seen.failed = Some(format!(
                    "the shell made no recycled item (0x{:08X})",
                    result.0 as u32
                ))
            }
        }
        Ok(())
    }

    fn StartOperations(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn FinishOperations(&self, _result: HRESULT) -> windows::core::Result<()> {
        Ok(())
    }
    fn PreRenameItem(&self, _: u32, _: Ref<IShellItem>, _: &PCWSTR) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostRenameItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: &PCWSTR,
        _: HRESULT,
        _: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PreMoveItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: Ref<IShellItem>,
        _: &PCWSTR,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostMoveItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: Ref<IShellItem>,
        _: &PCWSTR,
        _: HRESULT,
        _: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PreCopyItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: Ref<IShellItem>,
        _: &PCWSTR,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostCopyItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: Ref<IShellItem>,
        _: &PCWSTR,
        _: HRESULT,
        _: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PreNewItem(&self, _: u32, _: Ref<IShellItem>, _: &PCWSTR) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostNewItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: &PCWSTR,
        _: &PCWSTR,
        _: u32,
        _: HRESULT,
        _: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn UpdateProgress(&self, _: u32, _: u32) -> windows::core::Result<()> {
        Ok(())
    }
    fn ResetTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn PauseTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn ResumeTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
}

/// What `e` says, with its code: the shell's own codes have no text.
fn said(e: &windows::core::Error) -> String {
    let code = e.code().0 as u32;
    match e.message() {
        m if m.trim().is_empty() => format!("the shell failed with 0x{code:08X}"),
        m => format!("{} (0x{code:08X})", m.trim()),
    }
}

/// Where `item` is on disk.
fn file_path(item: &IShellItem) -> windows::core::Result<PathBuf> {
    // SAFETY: `GetDisplayName` returns a string this owns; it is copied,
    // then freed once with `CoTaskMemFree`.
    unsafe {
        let name = item.GetDisplayName(SIGDN_FILESYSPATH)?;
        let path = PathBuf::from(OsString::from_wide(name.as_wide()));
        CoTaskMemFree(Some(name.0 as *const _));
        Ok(path)
    }
}
