#include "toast_activator.h"

#include <NotificationActivationCallback.h>

#include <atomic>
#include <cwchar>
#include <new>
#include <utility>

#include "app_messages.h"
#include "flavor.h"
#include "utils.h"

namespace {

// The longest tag a toast gets (notifications_channel.cpp's ShortTag);
// anything longer isn't one of ours.
constexpr size_t kMaxTag = 64;

// How many clicks wait for the window at most.
constexpr size_t kMaxWaiting = 64;

class Callback final : public INotificationActivationCallback {
 public:
  explicit Callback(std::shared_ptr<ActivationQueue> queue)
      : queue_(std::move(queue)) {}

  IFACEMETHODIMP QueryInterface(REFIID iid, void** out) override {
    if (out == nullptr) return E_POINTER;
    if (iid == IID_IUnknown ||
        iid == __uuidof(INotificationActivationCallback)) {
      *out = static_cast<INotificationActivationCallback*>(this);
      AddRef();
      return S_OK;
    }
    *out = nullptr;
    return E_NOINTERFACE;
  }

  IFACEMETHODIMP_(ULONG) AddRef() override { return ++references_; }

  IFACEMETHODIMP_(ULONG) Release() override {
    ULONG left = --references_;
    if (left == 0) delete this;
    return left;
  }

  IFACEMETHODIMP Activate(LPCWSTR, LPCWSTR arguments,
                          const NOTIFICATION_USER_INPUT_DATA*, ULONG) override {
    std::string tag;
    if (arguments != nullptr && ::wcsnlen(arguments, kMaxTag + 1) <= kMaxTag) {
      tag = Utf8FromUtf16(arguments);
    }
    std::lock_guard<std::mutex> lock(queue_->mutex);
    if (queue_->window == nullptr || queue_->tags.size() >= kMaxWaiting) {
      return S_OK;
    }
    queue_->tags.push_back(tag);
    ::PostMessageW(queue_->window, kToastActivatedMessage, 0, 0);
    return S_OK;
  }

 private:
  ~Callback() = default;

  std::shared_ptr<ActivationQueue> queue_;
  std::atomic<ULONG> references_{1};
};

class Factory final : public IClassFactory {
 public:
  explicit Factory(std::shared_ptr<ActivationQueue> queue)
      : queue_(std::move(queue)) {}

  IFACEMETHODIMP QueryInterface(REFIID iid, void** out) override {
    if (out == nullptr) return E_POINTER;
    if (iid == IID_IUnknown || iid == IID_IClassFactory) {
      *out = static_cast<IClassFactory*>(this);
      AddRef();
      return S_OK;
    }
    *out = nullptr;
    return E_NOINTERFACE;
  }

  IFACEMETHODIMP_(ULONG) AddRef() override { return ++references_; }

  IFACEMETHODIMP_(ULONG) Release() override {
    ULONG left = --references_;
    if (left == 0) delete this;
    return left;
  }

  IFACEMETHODIMP CreateInstance(IUnknown* outer, REFIID iid,
                                void** out) override {
    if (out == nullptr) return E_POINTER;
    *out = nullptr;
    if (outer != nullptr) return CLASS_E_NOAGGREGATION;
    Callback* callback = new (std::nothrow) Callback(queue_);
    if (callback == nullptr) return E_OUTOFMEMORY;
    HRESULT made = callback->QueryInterface(iid, out);
    callback->Release();
    return made;
  }

  IFACEMETHODIMP LockServer(BOOL) override { return S_OK; }

 private:
  ~Factory() = default;

  std::shared_ptr<ActivationQueue> queue_;
  std::atomic<ULONG> references_{1};
};

}  // namespace

ToastActivator::ToastActivator(std::shared_ptr<ActivationQueue> queue) {
  CLSID clsid;
  if (FAILED(::CLSIDFromString(flavor::kToastActivator, &clsid))) return;
  Factory* factory = new (std::nothrow) Factory(std::move(queue));
  if (factory == nullptr) return;
  HRESULT done = ::CoRegisterClassObject(clsid, factory, CLSCTX_LOCAL_SERVER,
                                         REGCLS_MULTIPLEUSE, &cookie_);
  // COM keeps its own reference while it's registered.
  factory->Release();
  if (FAILED(done)) {
    cookie_ = 0;
    Log("toast clicks reach the app only while it runs: " +
        Win32ErrorMessage(static_cast<unsigned long>(done)));
  }
}

ToastActivator::~ToastActivator() {
  if (cookie_ != 0) ::CoRevokeClassObject(cookie_);
}

bool IsComStartArgument(const std::string& argument) {
  return _stricmp(argument.c_str(), "-Embedding") == 0 ||
         _stricmp(argument.c_str(), "/Embedding") == 0;
}
