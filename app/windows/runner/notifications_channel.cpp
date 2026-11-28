#include "notifications_channel.h"

#include <flutter/standard_method_codec.h>
#include <roapi.h>
#include <shellapi.h>
#include <windows.data.xml.dom.h>
#include <windows.ui.notifications.h>
#include <wrl/client.h>
#include <wrl/event.h>
#include <wrl/wrappers/corewrappers.h>

#include <cstdint>
#include <mutex>
#include <utility>
#include <variant>

#include "app_messages.h"
#include "app_paths.h"
#include "flavor.h"
#include "toast_activator.h"
#include "utils.h"

using ABI::Windows::Data::Xml::Dom::IXmlDocument;
using ABI::Windows::Data::Xml::Dom::IXmlDocumentIO;
using ABI::Windows::Foundation::ITypedEventHandler;
using ABI::Windows::UI::Notifications::IToastNotification;
using ABI::Windows::UI::Notifications::IToastNotification2;
using ABI::Windows::UI::Notifications::IToastNotification4;
using ABI::Windows::UI::Notifications::IToastNotificationFactory;
using ABI::Windows::UI::Notifications::IToastNotificationManagerStatics;
using ABI::Windows::UI::Notifications::IToastNotifier;
using ABI::Windows::UI::Notifications::NotificationSetting;
using ABI::Windows::UI::Notifications::ToastNotification;
using flutter::EncodableMap;
using flutter::EncodableValue;
using Microsoft::WRL::Callback;
using Microsoft::WRL::ComPtr;
using Microsoft::WRL::Wrappers::HStringReference;

namespace {

// How many shown toasts the app remembers the targets of.
constexpr size_t kRemembered = 200;

// Windows' limit on a toast's Tag and Group.
constexpr size_t kMaxTag = 64;

HStringReference Ref(const std::wstring& text) {
  return HStringReference(text.c_str(), static_cast<unsigned int>(text.size()));
}

std::string StringIn(const EncodableMap& map, const char* key) {
  auto found = map.find(EncodableValue(key));
  if (found == map.end()) return std::string();
  if (const auto* text = std::get_if<std::string>(&found->second)) return *text;
  return std::string();
}

bool BoolIn(const EncodableMap& map, const char* key) {
  auto found = map.find(EncodableValue(key));
  if (found == map.end()) return false;
  const auto* value = std::get_if<bool>(&found->second);
  return value != nullptr && *value;
}

// [text] for XML, as an element's text or an attribute's value.
std::wstring XmlEscaped(const std::wstring& text) {
  std::wstring out;
  out.reserve(text.size());
  for (wchar_t c : text) {
    switch (c) {
      case L'&':
        out += L"&amp;";
        break;
      case L'<':
        out += L"&lt;";
        break;
      case L'>':
        out += L"&gt;";
        break;
      case L'"':
        out += L"&quot;";
        break;
      case L'\'':
        out += L"&apos;";
        break;
      default:
        // XML 1.0 refuses most control characters; a line break stays.
        if (c < 0x20 && c != L'\n' && c != L'\t') {
          out += L' ';
        } else {
          out += c;
        }
    }
  }
  return out;
}

// A Tag or Group Windows takes: [text] itself when short enough, else a hash
// of it (FNV-1a, 64 bits, as hex), the same every time.
std::wstring ShortTag(const std::string& text) {
  if (!text.empty() && text.size() <= kMaxTag) {
    std::wstring wide = Utf16FromUtf8(text);
    if (!wide.empty() && wide.size() <= kMaxTag) return wide;
  }
  uint64_t hash = 14695981039346656037ULL;
  for (unsigned char c : text) {
    hash ^= c;
    hash *= 1099511628211ULL;
  }
  wchar_t hex[17];
  swprintf_s(hex, L"%016llx", static_cast<unsigned long long>(hash));
  return hex;
}

// Sets [name] of [key] in this account's registry to [value], unless it
// already is.
void SetUserString(const std::wstring& key, const wchar_t* name,
                   const std::wstring& value) {
  wchar_t kept[MAX_PATH * 4];
  DWORD size = sizeof(kept);
  if (::RegGetValueW(HKEY_CURRENT_USER, key.c_str(), name, RRF_RT_REG_SZ,
                     nullptr, kept, &size) == ERROR_SUCCESS &&
      value == kept) {
    return;
  }
  LSTATUS done = ::RegSetKeyValueW(
      HKEY_CURRENT_USER, key.c_str(), name, REG_SZ, value.c_str(),
      static_cast<DWORD>((value.size() + 1) * sizeof(wchar_t)));
  if (done != ERROR_SUCCESS) {
    Log("couldn't register for notifications: " + Win32ErrorMessage(done));
  }
}

// Registers the app's AppUserModelID for this account, so Windows shows its
// toasts without a Start menu shortcut: its name, its picture beside the
// app, and the COM class that takes their clicks (toast_activator.h), which
// Windows starts (through the launcher, installed) when the app isn't
// running. Written only where it isn't already so; a moved install fixes
// itself at its next start. The uninstaller removes both keys (README.md).
void RegisterAppId() {
  wchar_t module[MAX_PATH * 4];
  DWORD length = ::GetModuleFileNameW(nullptr, module,
                                      static_cast<DWORD>(std::size(module)));
  if (length == 0 || length >= std::size(module)) return;
  std::wstring exe(module, length);
  std::wstring folder = exe.substr(0, exe.find_last_of(L'\\'));
  std::wstring icon = folder + L"\\data\\" +
                      (flavor::kIsReal ? L"notification_icon.png"
                                       : L"notification_icon_dev.png");
  std::wstring app_key =
      std::wstring(L"Software\\Classes\\AppUserModelId\\") + flavor::kAppId;
  SetUserString(app_key, L"DisplayName", flavor::kAppName);
  SetUserString(app_key, L"IconUri", icon);
  // Installed, the launcher: it starts the current version, which a click
  // with the app closed still finds after an update deleted this one.
  std::wstring launcher = InstalledLauncher();
  SetUserString(std::wstring(L"Software\\Classes\\CLSID\\") +
                    flavor::kToastActivator + L"\\LocalServer32",
                nullptr, L"\"" + (launcher.empty() ? exe : launcher) + L"\"");
  SetUserString(app_key, L"CustomActivator", flavor::kToastActivator);
}

}  // namespace

struct NotificationsChannel::Toasts {
  ComPtr<IToastNotifier> notifier;
  ComPtr<IToastNotificationFactory> factory;
  bool tried = false;
  // The toasts shown, kept so their clicks reach this process.
  std::map<std::string, ComPtr<IToastNotification>> shown;
  std::shared_ptr<ActivationQueue> queue = std::make_shared<ActivationQueue>();
  // Takes clicks for Windows, the app running or not.
  std::unique_ptr<ToastActivator> activator;

  // The notifier and the factory, made once; false when Windows won't.
  bool Ready() {
    if (notifier && factory) return true;
    if (tried) return false;
    tried = true;
    ComPtr<IToastNotificationManagerStatics> manager;
    HRESULT made = ::RoGetActivationFactory(
        HStringReference(
            RuntimeClass_Windows_UI_Notifications_ToastNotificationManager)
            .Get(),
        IID_PPV_ARGS(&manager));
    if (SUCCEEDED(made)) {
      made = manager->CreateToastNotifierWithId(
          HStringReference(flavor::kAppId).Get(), &notifier);
    }
    if (SUCCEEDED(made)) {
      made = ::RoGetActivationFactory(
          HStringReference(RuntimeClass_Windows_UI_Notifications_ToastNotification)
              .Get(),
          IID_PPV_ARGS(&factory));
    }
    if (FAILED(made)) {
      Log("notifications can't be shown: " +
          Win32ErrorMessage(static_cast<unsigned long>(made)));
      notifier.Reset();
      factory.Reset();
      return false;
    }
    return true;
  }
};

NotificationsChannel::NotificationsChannel(flutter::BinaryMessenger* messenger,
                                           HWND window,
                                           const Presence& presence)
    : channel_(std::make_unique<flutter::MethodChannel<EncodableValue>>(
          messenger, "factory/notifications",
          &flutter::StandardMethodCodec::GetInstance())),
      window_(window),
      presence_(presence),
      toasts_(std::make_unique<Toasts>()) {
  toasts_->queue->window = window;
  RegisterAppId();
  toasts_->activator = std::make_unique<ToastActivator>(toasts_->queue);
  channel_->SetMethodCallHandler(
      [this](const auto& call, auto result) { Handle(call, std::move(result)); });
}

NotificationsChannel::~NotificationsChannel() {
  toasts_->activator = nullptr;
  // Clicks after this find no window to tell.
  std::lock_guard<std::mutex> lock(toasts_->queue->mutex);
  toasts_->queue->window = nullptr;
}

void NotificationsChannel::Handle(
    const flutter::MethodCall<EncodableValue>& call,
    std::unique_ptr<flutter::MethodResult<EncodableValue>> result) {
  const std::string& method = call.method_name();
  if (method == "listen") {
    listening_ = true;
    said_ = presence_.Using();
    result->Success(EncodableValue(said_));
    while (!clicks_.empty()) {
      channel_->InvokeMethod("clicked",
                             std::make_unique<EncodableValue>(clicks_.front()));
      clicks_.pop_front();
    }
    return;
  }
  if (method == "post") {
    if (const auto* map = std::get_if<EncodableMap>(call.arguments())) {
      Post(*map);
    }
    result->Success();
    return;
  }
  if (method == "permission" || method == "ask") {
    // Windows never asks the person: it's on unless they turned it off.
    result->Success(EncodableValue(Permission()));
    return;
  }
  if (method == "openSettings") {
    // Settings > System > Notifications.
    ::ShellExecuteW(nullptr, L"open", L"ms-settings:notifications", nullptr,
                    nullptr, SW_SHOWNORMAL);
    result->Success();
    return;
  }
  result->NotImplemented();
}

std::string NotificationsChannel::Permission() {
  if (!toasts_->Ready()) return "denied";
  NotificationSetting setting =
      ABI::Windows::UI::Notifications::NotificationSetting_DisabledForUser;
  if (FAILED(toasts_->notifier->get_Setting(&setting))) return "denied";
  return setting == ABI::Windows::UI::Notifications::NotificationSetting_Enabled
             ? "allowed"
             : "denied";
}

void NotificationsChannel::Post(const EncodableMap& notification) {
  if (!toasts_->Ready()) return;
  std::string id = StringIn(notification, "id");
  std::wstring title = Utf16FromUtf8(StringIn(notification, "title"));
  if (title.empty()) title = flavor::kAppName;
  std::wstring body = Utf16FromUtf8(StringIn(notification, "body"));
  std::wstring tag = ShortTag(id);
  std::wstring thread = ShortTag(StringIn(notification, "thread"));

  // The launch argument is the tag alone: what the click opens stays here.
  std::wstring xml = L"<toast launch=\"" + XmlEscaped(tag) +
                     L"\"><visual><binding template=\"ToastGeneric\"><text>" +
                     XmlEscaped(title) + L"</text><text>" + XmlEscaped(body) +
                     L"</text></binding></visual></toast>";
  ComPtr<IInspectable> made;
  HRESULT done = ::RoActivateInstance(
      HStringReference(RuntimeClass_Windows_Data_Xml_Dom_XmlDocument).Get(),
      &made);
  ComPtr<IXmlDocument> document;
  ComPtr<IXmlDocumentIO> io;
  if (SUCCEEDED(done)) done = made.As(&document);
  if (SUCCEEDED(done)) done = document.As(&io);
  if (SUCCEEDED(done)) done = io->LoadXml(Ref(xml).Get());
  ComPtr<IToastNotification> toast;
  if (SUCCEEDED(done)) {
    done = toasts_->factory->CreateToastNotification(document.Get(), &toast);
  }
  if (FAILED(done)) {
    Log("couldn't make a notification: " +
        Win32ErrorMessage(static_cast<unsigned long>(done)));
    return;
  }
  ComPtr<IToastNotification2> tagged;
  if (SUCCEEDED(toast.As(&tagged))) {
    tagged->put_Tag(Ref(tag).Get());
    tagged->put_Group(Ref(thread).Get());
  }
  if (BoolIn(notification, "urgent")) {
    ComPtr<IToastNotification4> prioritized;
    if (SUCCEEDED(toast.As(&prioritized))) {
      prioritized->put_Priority(
          ABI::Windows::UI::Notifications::ToastNotificationPriority_High);
    }
  }
  std::string key = Utf8FromUtf16(tag.c_str());
  // The activator takes the click when it's registered; the toast's own
  // event otherwise, which Windows raises only while this process runs.
  // Never both, or a click would open its target twice.
  if (!toasts_->activator->registered()) {
    std::shared_ptr<ActivationQueue> queue = toasts_->queue;
    EventRegistrationToken token{};
    toast->add_Activated(
        Callback<ITypedEventHandler<ToastNotification*, IInspectable*>>(
            [queue, key](IToastNotification*, IInspectable*) -> HRESULT {
              std::lock_guard<std::mutex> lock(queue->mutex);
              if (queue->window == nullptr) return S_OK;
              queue->tags.push_back(key);
              ::PostMessageW(queue->window, kToastActivatedMessage, 0, 0);
              return S_OK;
            })
            .Get(),
        &token);
  }
  done = toasts_->notifier->Show(toast.Get());
  if (FAILED(done)) {
    Log("Windows didn't show a notification: " +
        Win32ErrorMessage(static_cast<unsigned long>(done)));
    return;
  }

  auto target = notification.find(EncodableValue("target"));
  if (targets_.find(key) == targets_.end()) target_order_.push_back(key);
  targets_[key] =
      target != notification.end() ? target->second : EncodableValue(EncodableMap());
  toasts_->shown[key] = toast;
  while (target_order_.size() > kRemembered) {
    targets_.erase(target_order_.front());
    toasts_->shown.erase(target_order_.front());
    target_order_.pop_front();
  }
}

void NotificationsChannel::Changed() {
  bool now = presence_.Using();
  if (!listening_ || now == said_) return;
  said_ = now;
  channel_->InvokeMethod("using", std::make_unique<EncodableValue>(now));
}

void NotificationsChannel::Clicked(const std::string& tag) {
  if (::IsIconic(window_)) ::ShowWindow(window_, SW_RESTORE);
  ::SetForegroundWindow(window_);
  auto found = targets_.find(tag);
  EncodableValue target =
      found != targets_.end() ? found->second : EncodableValue(EncodableMap());
  if (listening_) {
    channel_->InvokeMethod("clicked", std::make_unique<EncodableValue>(target));
  } else {
    clicks_.push_back(target);
  }
}

void NotificationsChannel::ToastsActivated() {
  std::deque<std::string> tags;
  {
    std::lock_guard<std::mutex> lock(toasts_->queue->mutex);
    tags.swap(toasts_->queue->tags);
  }
  for (const auto& tag : tags) Clicked(tag);
}
