#include "drop_target.h"

#include <shellapi.h>
#include <shlguid.h>

#include "utils.h"

DropTarget::DropTarget(
    HWND window, std::function<void(int count)> over,
    std::function<void(std::vector<std::string> paths)> dropped)
    : window_(window), over_(std::move(over)), dropped_(std::move(dropped)) {
  // The shell's drag image, as Explorer shows it over any window that takes
  // drops. Optional: drops work without it.
  if (FAILED(::CoCreateInstance(CLSID_DragDropHelper, nullptr,
                                CLSCTX_INPROC_SERVER,
                                IID_PPV_ARGS(&helper_)))) {
    helper_ = nullptr;
  }
}

DropTarget::~DropTarget() {
  if (helper_ != nullptr) helper_->Release();
}

bool DropTarget::Register() {
  if (registered_) return true;
  HRESULT done = ::RegisterDragDrop(window_, this);
  if (FAILED(done)) {
    Log("files can't be dropped on the window: " +
        Win32ErrorMessage(static_cast<unsigned long>(done)));
    return false;
  }
  registered_ = true;
  return true;
}

void DropTarget::Revoke() {
  if (!registered_) return;
  ::RevokeDragDrop(window_);
  registered_ = false;
}

HRESULT STDMETHODCALLTYPE DropTarget::QueryInterface(REFIID riid, void** object) {
  if (object == nullptr) return E_POINTER;
  if (riid == IID_IUnknown || riid == IID_IDropTarget) {
    *object = static_cast<IDropTarget*>(this);
    AddRef();
    return S_OK;
  }
  *object = nullptr;
  return E_NOINTERFACE;
}

ULONG STDMETHODCALLTYPE DropTarget::AddRef() {
  return static_cast<ULONG>(::InterlockedIncrement(&references_));
}

ULONG STDMETHODCALLTYPE DropTarget::Release() {
  LONG left = ::InterlockedDecrement(&references_);
  if (left == 0) delete this;
  return static_cast<ULONG>(left);
}

std::vector<std::string> DropTarget::FilesIn(IDataObject* data) {
  std::vector<std::string> paths;
  if (data == nullptr) return paths;
  FORMATETC format{CF_HDROP, nullptr, DVASPECT_CONTENT, -1, TYMED_HGLOBAL};
  STGMEDIUM medium{};
  if (FAILED(data->GetData(&format, &medium))) return paths;
  auto drop = static_cast<HDROP>(::GlobalLock(medium.hGlobal));
  if (drop != nullptr) {
    UINT count = ::DragQueryFileW(drop, 0xFFFFFFFF, nullptr, 0);
    for (UINT i = 0; i < count; ++i) {
      UINT length = ::DragQueryFileW(drop, i, nullptr, 0);
      if (length == 0) continue;
      std::wstring path(static_cast<size_t>(length) + 1, L'\0');
      if (::DragQueryFileW(drop, i, path.data(), length + 1) == 0) continue;
      path.resize(length);
      std::string utf8 = Utf8FromUtf16(path.c_str());
      if (!utf8.empty()) paths.push_back(std::move(utf8));
    }
    ::GlobalUnlock(medium.hGlobal);
  }
  ::ReleaseStgMedium(&medium);
  return paths;
}

DWORD DropTarget::Effect() const {
  return takes_files_ && count_ > 0 ? DROPEFFECT_COPY : DROPEFFECT_NONE;
}

HRESULT STDMETHODCALLTYPE DropTarget::DragEnter(IDataObject* data, DWORD keys, POINTL point,
                              DWORD* effect) {
  count_ = static_cast<int>(FilesIn(data).size());
  // How many, for the window's drop overlay to say where they'd go, or why
  // they can't go anywhere here (lib/drop.dart).
  if (count_ > 0) over_(count_);
  *effect = Effect();
  if (helper_ != nullptr) {
    POINT at{point.x, point.y};
    helper_->DragEnter(window_, data, &at, *effect);
  }
  return S_OK;
}

HRESULT STDMETHODCALLTYPE DropTarget::DragOver(DWORD keys, POINTL point, DWORD* effect) {
  *effect = Effect();
  if (helper_ != nullptr) {
    POINT at{point.x, point.y};
    helper_->DragOver(&at, *effect);
  }
  return S_OK;
}

HRESULT STDMETHODCALLTYPE DropTarget::DragLeave() {
  if (helper_ != nullptr) helper_->DragLeave();
  if (count_ > 0) over_(0);
  count_ = 0;
  return S_OK;
}

HRESULT STDMETHODCALLTYPE DropTarget::Drop(IDataObject* data, DWORD keys, POINTL point,
                         DWORD* effect) {
  std::vector<std::string> paths = FilesIn(data);
  count_ = static_cast<int>(paths.size());
  *effect = Effect();
  if (helper_ != nullptr) {
    POINT at{point.x, point.y};
    helper_->Drop(data, &at, *effect);
  }
  if (*effect == DROPEFFECT_NONE) {
    over_(0);
  } else {
    dropped_(std::move(paths));
  }
  count_ = 0;
  return S_OK;
}
