#ifndef RUNNER_CORE_CONNECTION_STORE_H_
#define RUNNER_CORE_CONNECTION_STORE_H_

// Where this PC keeps its connections (app/lib/connections.dart's
// DeviceConnectionStorage, on factory/device: connections, setConnections,
// resetConnections): what the Dart side hands over, kept as JSON in one
// secret blob only this person's Windows account can open (DpapiFile, user
// DPAPI, on Windows). The same rules as the Mac's and iPhone's
// ConnectionStore.swift:
// - a store that couldn't be read is never written over with nothing: before
//   the first read, and after a failed one, every write is refused
//   (credentials_read_failed) until a read succeeds or the person starts
//   over;
// - no fallback: a blob that can't be opened is an error the app shows,
//   never a reason to keep them in plain text or begin again by itself
//   (windows.md section 3, Vault/storage).
// The error codes are the app's (and Android's) names, which
// connections.dart passes on as ConnectionStorageFailure.code.
// Standard C++17 only, so the tests build it anywhere.

#include <functional>
#include <memory>
#include <optional>
#include <string>
#include <variant>

#include "core/json.h"

namespace claktory {

// One secret blob, kept somewhere only this app's person can read.
class SecretBlob {
 public:
  struct ReadResult {
    enum class Status {
      // [data] holds it.
      kFound,
      // Nothing was ever kept (or it was removed).
      kMissing,
      // Can't be read now, but may be later (a sharing violation, a full
      // disk, a locked profile): worth trying again.
      kUnavailable,
      // There, but not what was written, or not openable by this account
      // (another user's blob, a reset DPAPI master key, a damaged file).
      kUnreadable,
    };
    Status status = Status::kMissing;
    std::string data;
    // What the system said, for the log and the error's message.
    std::string error;
  };

  virtual ~SecretBlob() = default;
  virtual ReadResult Read() = 0;
  // An error's words, or nullopt once [data] is kept (whole, or not at all).
  virtual std::optional<std::string> Write(const std::string& data) = 0;
  // Removes it; nothing there is no failure.
  virtual std::optional<std::string> Erase() = 0;
};

struct StoreFailure {
  std::string code;
  std::string message;
};

class ConnectionStore {
 public:
  ConnectionStore(std::unique_ptr<SecretBlob> blob,
                  std::function<void(const std::string&)> log);

  // What's kept: an empty map only when nothing ever was.
  // credentials_unavailable (worth trying again) or credentials_unreadable
  // (there, but not readable as connections) when it can't be read.
  std::variant<Json, StoreFailure> Read();

  // Keeps [connections], a map; credentials_read_failed before any read has
  // succeeded or while the last read failed, credentials_write_failed when it
  // can't be kept.
  std::optional<StoreFailure> Write(const Json& connections);

  // Starts over, at the person's word: whatever's kept goes and writing is
  // allowed again; credentials_reset_failed when it can't be removed.
  std::optional<StoreFailure> Reset();

  // Whether writes are refused: nothing read yet, or the last read failed.
  bool read_failed() const { return !read_ || read_failed_; }

 private:
  std::unique_ptr<SecretBlob> blob_;
  std::function<void(const std::string&)> log_;
  // A read has succeeded (or the person started over) since the app started.
  bool read_ = false;
  bool read_failed_ = false;
};

}  // namespace claktory

#endif  // RUNNER_CORE_CONNECTION_STORE_H_
