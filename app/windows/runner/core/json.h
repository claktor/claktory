#ifndef RUNNER_CORE_JSON_H_
#define RUNNER_CORE_JSON_H_

// A small JSON value, writer and reader for what the runner keeps on disk
// (the connections, the drafts, the window's place): the same JSON the Mac's
// runner keeps (ConnectionStore.swift encodes with JSONSerialization).
// Standard C++17 only, so the tests build it anywhere (test/README.md).

#include <cstdint>
#include <optional>
#include <string>
#include <utility>
#include <vector>

namespace claktory {

class Json {
 public:
  enum class Kind { kNull, kBool, kInt, kDouble, kString, kList, kMap };
  using List = std::vector<Json>;
  // In the order written; keys are unique (Set replaces).
  using Map = std::vector<std::pair<std::string, Json>>;

  Json() = default;
  static Json Bool(bool value);
  static Json Int(int64_t value);
  static Json Double(double value);
  static Json String(std::string value);
  static Json MakeList(List items = {});
  static Json MakeMap(Map entries = {});

  Kind kind() const { return kind_; }
  bool is_null() const { return kind_ == Kind::kNull; }
  bool is_map() const { return kind_ == Kind::kMap; }
  bool is_list() const { return kind_ == Kind::kList; }
  bool is_string() const { return kind_ == Kind::kString; }

  bool bool_value() const { return bool_; }
  int64_t int_value() const { return int_; }
  double double_value() const { return double_; }
  const std::string& string_value() const { return string_; }
  const List& list() const { return list_; }
  List& list() { return list_; }
  const Map& map() const { return map_; }

  // A map's value for [key]; null when there's none or this isn't a map.
  const Json* Find(const std::string& key) const;
  // Sets [key] in a map (a null value becomes an empty map first).
  void Set(const std::string& key, Json value);

  bool operator==(const Json& other) const;
  bool operator!=(const Json& other) const { return !(*this == other); }

 private:
  Kind kind_ = Kind::kNull;
  bool bool_ = false;
  int64_t int_ = 0;
  double double_ = 0;
  std::string string_;
  List list_;
  Map map_;
};

// [value] as compact JSON, in UTF-8. Doubles that aren't finite (JSON has no
// NaN or infinity) are written as null.
std::string WriteJson(const Json& value);

// [text] (UTF-8) as JSON; nullopt when it isn't exactly one JSON value, or
// nests deeper than 64. [error] says where it failed.
std::optional<Json> ReadJson(const std::string& text,
                             std::string* error = nullptr);

}  // namespace claktory

#endif  // RUNNER_CORE_JSON_H_
