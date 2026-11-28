// Tests of the runner's portable core (runner/core): JSON, the connection
// store's rules, and the window's geometry. No test framework: each CHECK
// prints what failed, and the exit status counts failures. Builds anywhere
// with a C++17 compiler (README.md says how).

#include <cmath>
#include <cstdio>
#include <memory>
#include <string>

#include "core/connection_store.h"
#include "core/json.h"
#include "core/command_line.h"
#include "core/launcher_plan.h"
#include "core/window_geometry.h"

using claktory::ConnectionStore;
using claktory::Json;
using claktory::Rect;
using claktory::SecretBlob;
using claktory::StoreFailure;

namespace {

int failures = 0;
int checks = 0;

#define CHECK(condition)                                               \
  do {                                                                 \
    ++checks;                                                          \
    if (!(condition)) {                                                \
      ++failures;                                                      \
      std::printf("FAILED %s:%d: %s\n", __FILE__, __LINE__, #condition); \
    }                                                                  \
  } while (0)

// ---- JSON ----

void JsonRoundTrips() {
  Json value = Json::MakeMap();
  value.Set("current", Json::String("factory://abc"));
  value.Set("saved", Json::MakeList({Json::String("a"), Json::String("b")}));
  Json keys = Json::MakeMap();
  keys.Set("factory://abc", Json::String("00ff"));
  value.Set("keys", keys);
  value.Set("n", Json::Int(-42));
  value.Set("x", Json::Double(2.0));
  value.Set("ok", Json::Bool(true));
  value.Set("none", Json());
  std::string text = claktory::WriteJson(value);
  std::optional<Json> back = claktory::ReadJson(text);
  CHECK(back.has_value());
  CHECK(back.has_value() && *back == value);
  // 2.0 stays a double.
  CHECK(back.has_value() && back->Find("x")->kind() == Json::Kind::kDouble);
  CHECK(back.has_value() && back->Find("n")->kind() == Json::Kind::kInt);
}

void JsonEscapes() {
  Json value = Json::String("quote \" backslash \\ tab \t nl \n \x01 é 🙂");
  std::string text = claktory::WriteJson(value);
  CHECK(text.find("\\u0001") != std::string::npos);
  std::optional<Json> back = claktory::ReadJson(text);
  CHECK(back.has_value() && *back == value);
  // \u escapes, a surrogate pair among them.
  std::optional<Json> read =
      claktory::ReadJson("\"\\u00e9\\ud83d\\ude42\\/\"");
  CHECK(read.has_value() && read->string_value() == "é🙂/");
  CHECK(!claktory::ReadJson("\"\\ud83d\"").has_value());
  CHECK(!claktory::ReadJson("\"\\ude42\"").has_value());
}

void JsonRefusesWhatIsntJson() {
  const char* bad[] = {"",       "{",         "{\"a\":}", "[1,]",  "01",
                       "1.",     "tru",       "{} x",     "\"\x01\"",
                       "{'a':1}", "[1 2]",    "nul",      "-"};
  for (const char* text : bad) {
    std::string error;
    bool refused = !claktory::ReadJson(text, &error).has_value();
    CHECK(refused);
    if (refused) CHECK(!error.empty());
  }
  // Too deep.
  std::string deep(100, '[');
  deep += std::string(100, ']');
  CHECK(!claktory::ReadJson(deep).has_value());
  // Whitespace around is fine.
  CHECK(claktory::ReadJson(" \n{ \"a\" : [ 1 , 2.5e3 , null ] }\t").has_value());
}

void JsonNumbers() {
  CHECK(claktory::ReadJson("9223372036854775807")->int_value() ==
        INT64_C(9223372036854775807));
  // Past 64 bits: a double.
  CHECK(claktory::ReadJson("92233720368547758070")->kind() ==
        Json::Kind::kDouble);
  CHECK(claktory::WriteJson(Json::Double(NAN)) == "null");
  CHECK(claktory::WriteJson(Json::Double(0.5)) == "0.5");
}

void JsonMapsCompareWithoutOrder() {
  CHECK(*claktory::ReadJson("{\"a\":1,\"b\":2}") ==
        *claktory::ReadJson("{\"b\":2,\"a\":1}"));
  CHECK(*claktory::ReadJson("{\"a\":1}") != *claktory::ReadJson("{\"a\":2}"));
  // A repeated key: the last wins, once.
  std::optional<Json> repeated = claktory::ReadJson("{\"a\":1,\"a\":2}");
  CHECK(repeated->map().size() == 1 && repeated->Find("a")->int_value() == 2);
}

// ---- The connection store ----

class MemoryBlob : public SecretBlob {
 public:
  ReadResult next;
  bool has = false;
  std::string data;
  std::optional<std::string> write_error;
  std::optional<std::string> erase_error;
  int writes = 0;

  ReadResult Read() override {
    if (next.status == ReadResult::Status::kUnavailable ||
        next.status == ReadResult::Status::kUnreadable) {
      return next;
    }
    ReadResult result;
    if (has) {
      result.status = ReadResult::Status::kFound;
      result.data = data;
    }
    return result;
  }
  std::optional<std::string> Write(const std::string& bytes) override {
    if (write_error) return write_error;
    ++writes;
    has = true;
    data = bytes;
    return std::nullopt;
  }
  std::optional<std::string> Erase() override {
    if (erase_error) return erase_error;
    has = false;
    data.clear();
    return std::nullopt;
  }
};

struct Store {
  MemoryBlob* blob;
  std::unique_ptr<ConnectionStore> store;
  std::string log;
};

Store MakeStore() {
  auto blob = std::make_unique<MemoryBlob>();
  Store made;
  made.blob = blob.get();
  made.store = std::make_unique<ConnectionStore>(
      std::move(blob), [](const std::string&) {});
  return made;
}

Json SomeConnections() {
  return *claktory::ReadJson(
      "{\"current\":\"factory://x\",\"saved\":[\"factory://x\"],"
      "\"keys\":{\"factory://x\":\"k\"},\"account\":{}}");
}

void StoreStartsEmpty() {
  Store s = MakeStore();
  auto read = s.store->Read();
  CHECK(std::holds_alternative<Json>(read));
  CHECK(std::get<Json>(read) == Json::MakeMap());
}

void StoreWritesNothingBeforeItsRead() {
  Store s = MakeStore();
  s.blob->has = true;
  s.blob->data = "kept";
  auto refused = s.store->Write(SomeConnections());
  CHECK(refused.has_value() && refused->code == "credentials_read_failed");
  CHECK(s.blob->writes == 0 && s.blob->data == "kept");
  CHECK(s.store->read_failed());
}

void StoreKeepsWhatItsGiven() {
  Store s = MakeStore();
  s.store->Read();
  CHECK(!s.store->Write(SomeConnections()).has_value());
  auto read = s.store->Read();
  CHECK(std::holds_alternative<Json>(read) &&
        std::get<Json>(read) == SomeConnections());
}

void StoreNeverWritesOverWhatItCouldntRead() {
  Store s = MakeStore();
  s.blob->has = true;
  s.blob->data = "kept";
  s.blob->next.status = SecretBlob::ReadResult::Status::kUnavailable;
  s.blob->next.error = "sharing violation";
  auto read = s.store->Read();
  CHECK(std::holds_alternative<StoreFailure>(read) &&
        std::get<StoreFailure>(read).code == "credentials_unavailable");
  auto refused = s.store->Write(SomeConnections());
  CHECK(refused.has_value() && refused->code == "credentials_read_failed");
  CHECK(s.blob->writes == 0 && s.blob->data == "kept");
  // A read that works again lets writes through.
  s.blob->next.status = SecretBlob::ReadResult::Status::kMissing;
  s.blob->data = "{}";
  CHECK(std::holds_alternative<Json>(s.store->Read()));
  CHECK(!s.store->Write(SomeConnections()).has_value());
}

void StoreSaysWhatsUnreadable() {
  Store s = MakeStore();
  s.blob->next.status = SecretBlob::ReadResult::Status::kUnreadable;
  s.blob->next.error = "The data is invalid.";
  auto read = s.store->Read();
  CHECK(std::holds_alternative<StoreFailure>(read) &&
        std::get<StoreFailure>(read).code == "credentials_unreadable");
  // Opened, but not connections: unreadable too, and nothing written over.
  Store t = MakeStore();
  t.blob->has = true;
  t.blob->data = "[1,2]";
  auto list = t.store->Read();
  CHECK(std::holds_alternative<StoreFailure>(list) &&
        std::get<StoreFailure>(list).code == "credentials_unreadable");
  CHECK(t.store->Write(SomeConnections())->code == "credentials_read_failed");
  CHECK(t.blob->data == "[1,2]");
  t.blob->data = "not json";
  CHECK(std::holds_alternative<StoreFailure>(t.store->Read()));
}

void StoreStartsOverAtThePersonsWord() {
  Store s = MakeStore();
  s.blob->has = true;
  s.blob->data = "garbage";
  CHECK(std::holds_alternative<StoreFailure>(s.store->Read()));
  CHECK(!s.store->Reset().has_value());
  CHECK(!s.blob->has);
  CHECK(!s.store->read_failed());
  CHECK(!s.store->Write(SomeConnections()).has_value());
  // A reset that fails says so, and still refuses writes.
  Store t = MakeStore();
  t.blob->has = true;
  t.blob->data = "garbage";
  t.store->Read();
  t.blob->erase_error = "access denied";
  auto reset = t.store->Reset();
  CHECK(reset.has_value() && reset->code == "credentials_reset_failed");
  CHECK(t.store->read_failed());
}

void StoreSaysWhenItCantWrite() {
  Store s = MakeStore();
  s.store->Read();
  s.blob->write_error = "disk full";
  auto failed = s.store->Write(SomeConnections());
  CHECK(failed.has_value() && failed->code == "credentials_write_failed" &&
        failed->message == "disk full");
  s.blob->write_error.reset();
  auto not_map = s.store->Write(Json::MakeList());
  CHECK(not_map.has_value() && not_map->code == "credentials_write_failed");
}

// ---- The window's geometry ----

void DefaultFrameIsCenteredAndCapped() {
  // A 1920 x 1040 work area at 100%: 78% x 85% is 1498 x 884, capped to
  // 1440 wide.
  Rect frame = claktory::DefaultFrame(Rect{0, 0, 1920, 1040}, 1.0);
  CHECK(frame.width() == 1440);
  CHECK(frame.height() == 884);
  CHECK(frame.left == 240 && frame.top == 78);
  // At 150% on a 4K screen the caps scale too.
  Rect big = claktory::DefaultFrame(Rect{0, 0, 3840, 2100}, 1.5);
  CHECK(big.width() == 2160 && big.height() == 1410);
  // A small screen still gets the smallest size.
  Rect small = claktory::DefaultFrame(Rect{100, 0, 900, 560}, 1.0);
  CHECK(small.width() == 800 && small.height() == 500);
  // On a second screen to the left.
  Rect left = claktory::DefaultFrame(Rect{-1920, 0, 0, 1040}, 1.0);
  CHECK(left.left == -1680);
}

void SavedFrameRoundTrips() {
  claktory::SavedFrame saved;
  saved.frame = Rect{-100, 20, 1300, 900};
  saved.maximized = true;
  auto back = claktory::SavedFrameFromJson(
      *claktory::ReadJson(claktory::WriteJson(claktory::SavedFrameToJson(saved))));
  CHECK(back.has_value() && back->frame == saved.frame && back->maximized);
  CHECK(!claktory::SavedFrameFromJson(*claktory::ReadJson("{\"left\":1}")));
  CHECK(!claktory::SavedFrameFromJson(*claktory::ReadJson(
      "{\"left\":10,\"top\":0,\"right\":5,\"bottom\":100}")));
  CHECK(!claktory::SavedFrameFromJson(*claktory::ReadJson("[]")));
}

void SavedFrameMustStillBeOnAScreen() {
  std::vector<Rect> screens = {Rect{0, 0, 1920, 1040}};
  CHECK(claktory::FrameStillFits(Rect{100, 100, 1300, 900}, screens, 1.0));
  // Its band off the top, or on a screen unplugged since.
  CHECK(!claktory::FrameStillFits(Rect{100, -400, 1300, 600}, screens, 1.0));
  CHECK(!claktory::FrameStillFits(Rect{2000, 100, 3200, 900}, screens, 1.0));
  // Mostly off the right edge, but enough of its band showing.
  CHECK(claktory::FrameStillFits(Rect{1700, 100, 2900, 900}, screens, 1.0));
  // Smaller than the smallest size.
  CHECK(!claktory::FrameStillFits(Rect{100, 100, 500, 400}, screens, 1.0));
  // A second screen holds it.
  screens.push_back(Rect{1920, 0, 3840, 1080});
  CHECK(claktory::FrameStillFits(Rect{2000, 100, 3200, 900}, screens, 1.0));
}

// ---- the launcher's rules ----

void LauncherVersionsAreDigitsAndDots() {
  CHECK(claktory::IsAppVersion("0.1.27"));
  CHECK(claktory::IsAppVersion("1"));
  CHECK(claktory::IsAppVersion("2026.11.19.1958"));
  CHECK(!claktory::IsAppVersion(""));
  CHECK(!claktory::IsAppVersion("."));
  CHECK(!claktory::IsAppVersion(".1"));
  CHECK(!claktory::IsAppVersion("1."));
  CHECK(!claktory::IsAppVersion("1..2"));
  CHECK(!claktory::IsAppVersion(".."));
  CHECK(!claktory::IsAppVersion("..\\..\\x"));
  CHECK(!claktory::IsAppVersion("C:\\x"));
  CHECK(!claktory::IsAppVersion("0.1.27\\..\\0.1.26"));
  CHECK(!claktory::IsAppVersion("0.1.27 "));
  CHECK(!claktory::IsAppVersion("v0.1.27"));
  CHECK(!claktory::IsAppVersion(std::string(33, '1')));
  CHECK(claktory::IsAppVersion(std::string(32, '1')));
}

void LauncherReadsOnlyAVersionFromItsPointer() {
  CHECK(claktory::PointedVersion("0.1.27") == "0.1.27");
  CHECK(claktory::PointedVersion("0.1.27\r\n") == "0.1.27");
  CHECK(claktory::PointedVersion("  0.1.27\n\n") == "0.1.27");
  CHECK(claktory::PointedVersion("").empty());
  CHECK(claktory::PointedVersion("\n").empty());
  CHECK(claktory::PointedVersion("0.1.27\n0.1.26").empty());
  CHECK(claktory::PointedVersion("..").empty());
  CHECK(claktory::PointedVersion(std::string("0.1\0.27", 7)).empty());
  // A byte-order mark is no part of a version.
  CHECK(claktory::PointedVersion("\xEF\xBB\xBF" "0.1.27").empty());
}

void LauncherCountsOnlyAFailedExitAsFailing() {
  CHECK(!claktory::FailedToStart(false, 0));
  CHECK(!claktory::FailedToStart(false, 1));
  // Handed over to the app already running.
  CHECK(!claktory::FailedToStart(true, 0));
  CHECK(claktory::FailedToStart(true, 1));
  CHECK(claktory::FailedToStart(true, 0xC0000005ul));
}

void LauncherTakesTheProcessToWaitFor() {
  std::wstring arguments = L"--after 4321 claktory://open";
  CHECK(claktory::TakeAfter(&arguments) == 4321);
  CHECK(arguments == L"claktory://open");
  arguments = L"--after 4321";
  CHECK(claktory::TakeAfter(&arguments) == 4321);
  CHECK(arguments.empty());
  for (const wchar_t* kept :
       {L"", L"claktory://open --after 4321", L"--after", L"--after x",
        L"--after 0", L"--after 12x", L"--after 4294967296",
        L"--after 99999999999", L"--afterward 12"}) {
    arguments = kept;
    CHECK(claktory::TakeAfter(&arguments) == 0);
    CHECK(arguments == kept);
  }
  arguments = L"--after 4294967295";
  CHECK(claktory::TakeAfter(&arguments) == 4294967295ul);
}

void TheAppFindsItsLauncherTwoFoldersUp() {
  CHECK(claktory::LauncherFor(
            L"C:\\Users\\ann\\AppData\\Local\\Programs\\Claktory\\app\\0.1.27\\Claktory.exe") ==
        L"C:\\Users\\ann\\AppData\\Local\\Programs\\Claktory\\Claktory.exe");
  CHECK(claktory::LauncherFor(L"D:\\Claktory\\APP\\0.1.27\\Claktory.exe") ==
        L"D:\\Claktory\\Claktory.exe");
  for (const wchar_t* elsewhere :
       {L"C:\\build\\app\\build\\windows\\x64\\runner\\Release\\Claktory.exe",
        L"C:\\Claktory\\app\\v0.1.27\\Claktory.exe",
        L"C:\\Claktory\\app\\0.1.27.partial\\Claktory.exe",
        L"C:\\Claktory\\apps\\0.1.27\\Claktory.exe",
        L"app\\0.1.27\\Claktory.exe", L"Claktory.exe", L""}) {
    CHECK(claktory::LauncherFor(elsewhere).empty());
  }
}

void CommandLinesQuoteAsWindowsReadsThem() {
  CHECK(claktory::CommandLine(L"C:\\Claktory\\factoryd.exe",
                              {L"install", L"--check"}) ==
        L"\"C:\\Claktory\\factoryd.exe\" install --check");
  CHECK(claktory::CommandLine(L"C:\\Program Files\\f.exe", {}) ==
        L"\"C:\\Program Files\\f.exe\"");
  CHECK(claktory::CommandLine(L"f.exe", {L""}) == L"\"f.exe\" \"\"");
  CHECK(claktory::CommandLine(L"f.exe", {L"a b"}) == L"\"f.exe\" \"a b\"");
  CHECK(claktory::CommandLine(L"f.exe", {L"say \"hi\""}) ==
        L"\"f.exe\" \"say \\\"hi\\\"\"");
  // Backslashes are literal unless a quote follows them.
  CHECK(claktory::CommandLine(L"f.exe", {L"C:\\a\\b"}) ==
        L"\"f.exe\" C:\\a\\b");
  CHECK(claktory::CommandLine(L"f.exe", {L"C:\\my dir\\"}) ==
        L"\"f.exe\" \"C:\\my dir\\\\\"");
  CHECK(claktory::CommandLine(L"f.exe", {L"a\\\"b"}) ==
        L"\"f.exe\" \"a\\\\\\\"b\"");
  CHECK(claktory::CommandLine(L"f.exe", {L"tab\there"}) ==
        L"\"f.exe\" \"tab\there\"");
}

void LauncherDeletesOnlyVersionsNeitherPointerNames() {
  auto stale = claktory::StaleVersions(
      {"0.1.25", "0.1.26", "0.1.27", "launcher.log", "..", "x", "0.1.28.tmp"},
      "0.1.27", "0.1.26");
  CHECK(stale.size() == 1 && stale[0] == "0.1.25");
  // With no previous, only current is kept.
  stale = claktory::StaleVersions({"0.1.26", "0.1.27"}, "0.1.27", "");
  CHECK(stale.size() == 1 && stale[0] == "0.1.26");
  // With no current, the launcher deletes nothing (it isn't called), but
  // the rule itself still never names a non-version.
  stale = claktory::StaleVersions({"current", "previous"}, "", "");
  CHECK(stale.empty());
}

}  // namespace

int main() {
  JsonRoundTrips();
  JsonEscapes();
  JsonRefusesWhatIsntJson();
  JsonNumbers();
  JsonMapsCompareWithoutOrder();
  StoreStartsEmpty();
  StoreWritesNothingBeforeItsRead();
  StoreKeepsWhatItsGiven();
  StoreNeverWritesOverWhatItCouldntRead();
  StoreSaysWhatsUnreadable();
  StoreStartsOverAtThePersonsWord();
  StoreSaysWhenItCantWrite();
  DefaultFrameIsCenteredAndCapped();
  SavedFrameRoundTrips();
  SavedFrameMustStillBeOnAScreen();
  LauncherVersionsAreDigitsAndDots();
  LauncherReadsOnlyAVersionFromItsPointer();
  LauncherCountsOnlyAFailedExitAsFailing();
  LauncherTakesTheProcessToWaitFor();
  TheAppFindsItsLauncherTwoFoldersUp();
  CommandLinesQuoteAsWindowsReadsThem();
  LauncherDeletesOnlyVersionsNeitherPointerNames();
  std::printf("%d checks, %d failed\n", checks, failures);
  return failures == 0 ? 0 : 1;
}
