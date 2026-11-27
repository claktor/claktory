#include "core/connection_store.h"

#include <utility>

namespace claktory {

ConnectionStore::ConnectionStore(std::unique_ptr<SecretBlob> blob,
                                 std::function<void(const std::string&)> log)
    : blob_(std::move(blob)), log_(std::move(log)) {}

std::variant<Json, StoreFailure> ConnectionStore::Read() {
  SecretBlob::ReadResult kept = blob_->Read();
  switch (kept.status) {
    case SecretBlob::ReadResult::Status::kMissing:
      read_ = true;
      read_failed_ = false;
      return Json::MakeMap();
    case SecretBlob::ReadResult::Status::kUnavailable:
      read_failed_ = true;
      log_("couldn't read them: " + kept.error);
      return StoreFailure{"credentials_unavailable", kept.error};
    case SecretBlob::ReadResult::Status::kUnreadable:
      read_failed_ = true;
      log_("what's kept can't be opened: " + kept.error);
      return StoreFailure{"credentials_unreadable", kept.error};
    case SecretBlob::ReadResult::Status::kFound:
      break;
  }
  std::string error;
  std::optional<Json> connections = ReadJson(kept.data, &error);
  if (!connections || !connections->is_map()) {
    read_failed_ = true;
    log_("what's kept can't be read as connections" +
         (error.empty() ? std::string() : ": " + error));
    return StoreFailure{"credentials_unreadable",
                        "What's kept isn't readable as connections."};
  }
  read_ = true;
  read_failed_ = false;
  return std::move(*connections);
}

std::optional<StoreFailure> ConnectionStore::Write(const Json& connections) {
  if (!read_) {
    // Fugo (45231): a write before the first read never lands on a store
    // that's there but can't be read.
    return StoreFailure{"credentials_read_failed",
                        "They haven't been read yet, so they aren't written over."};
  }
  if (read_failed_) {
    return StoreFailure{"credentials_read_failed",
                        "They couldn't be read, so they aren't written over."};
  }
  if (!connections.is_map()) {
    return StoreFailure{"credentials_write_failed",
                        "Connections must be a map."};
  }
  if (std::optional<std::string> error = blob_->Write(WriteJson(connections))) {
    log_("couldn't write them: " + *error);
    return StoreFailure{"credentials_write_failed", *error};
  }
  return std::nullopt;
}

std::optional<StoreFailure> ConnectionStore::Reset() {
  if (std::optional<std::string> error = blob_->Erase()) {
    log_("couldn't remove them: " + *error);
    return StoreFailure{"credentials_reset_failed", *error};
  }
  read_ = true;
  read_failed_ = false;
  log_("started over");
  return std::nullopt;
}

}  // namespace claktory
