#include "encodable_json.h"

#include <cstdint>
#include <variant>

using claktory::Json;
using flutter::EncodableList;
using flutter::EncodableMap;
using flutter::EncodableValue;

namespace {

constexpr int kMaxDepth = 64;

std::optional<Json> Convert(const EncodableValue& value, int depth,
                            std::string* error) {
  if (depth > kMaxDepth) {
    *error = "nested too deep";
    return std::nullopt;
  }
  if (std::holds_alternative<std::monostate>(value)) return Json();
  if (const auto* boolean = std::get_if<bool>(&value)) {
    return Json::Bool(*boolean);
  }
  if (const auto* small = std::get_if<int32_t>(&value)) {
    return Json::Int(*small);
  }
  if (const auto* big = std::get_if<int64_t>(&value)) {
    return Json::Int(*big);
  }
  if (const auto* number = std::get_if<double>(&value)) {
    return Json::Double(*number);
  }
  if (const auto* text = std::get_if<std::string>(&value)) {
    return Json::String(*text);
  }
  if (const auto* list = std::get_if<EncodableList>(&value)) {
    Json out = Json::MakeList();
    for (const auto& item : *list) {
      std::optional<Json> converted = Convert(item, depth + 1, error);
      if (!converted) return std::nullopt;
      out.list().push_back(std::move(*converted));
    }
    return out;
  }
  if (const auto* map = std::get_if<EncodableMap>(&value)) {
    Json out = Json::MakeMap();
    for (const auto& entry : *map) {
      const auto* key = std::get_if<std::string>(&entry.first);
      if (key == nullptr) {
        *error = "a map key that isn't a string";
        return std::nullopt;
      }
      std::optional<Json> converted = Convert(entry.second, depth + 1, error);
      if (!converted) return std::nullopt;
      out.Set(*key, std::move(*converted));
    }
    return out;
  }
  *error = "a value JSON can't hold (bytes or a typed list)";
  return std::nullopt;
}

}  // namespace

std::optional<Json> JsonFromEncodable(const EncodableValue& value,
                                      std::string* error) {
  std::string why;
  std::optional<Json> json = Convert(value, 0, &why);
  if (!json && error != nullptr) *error = why;
  return json;
}

EncodableValue EncodableFromJson(const Json& json) {
  switch (json.kind()) {
    case Json::Kind::kNull:
      return EncodableValue();
    case Json::Kind::kBool:
      return EncodableValue(json.bool_value());
    case Json::Kind::kInt: {
      int64_t value = json.int_value();
      // The codec sends small ints as Dart ints either way.
      if (value >= INT32_MIN && value <= INT32_MAX) {
        return EncodableValue(static_cast<int32_t>(value));
      }
      return EncodableValue(value);
    }
    case Json::Kind::kDouble:
      return EncodableValue(json.double_value());
    case Json::Kind::kString:
      return EncodableValue(json.string_value());
    case Json::Kind::kList: {
      EncodableList list;
      list.reserve(json.list().size());
      for (const auto& item : json.list()) {
        list.push_back(EncodableFromJson(item));
      }
      return EncodableValue(std::move(list));
    }
    case Json::Kind::kMap: {
      EncodableMap map;
      for (const auto& entry : json.map()) {
        map[EncodableValue(entry.first)] = EncodableFromJson(entry.second);
      }
      return EncodableValue(std::move(map));
    }
  }
  return EncodableValue();
}
