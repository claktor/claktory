#include "core/json.h"

#include <cerrno>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>

namespace claktory {

Json Json::Bool(bool value) {
  Json json;
  json.kind_ = Kind::kBool;
  json.bool_ = value;
  return json;
}

Json Json::Int(int64_t value) {
  Json json;
  json.kind_ = Kind::kInt;
  json.int_ = value;
  return json;
}

Json Json::Double(double value) {
  Json json;
  json.kind_ = Kind::kDouble;
  json.double_ = value;
  return json;
}

Json Json::String(std::string value) {
  Json json;
  json.kind_ = Kind::kString;
  json.string_ = std::move(value);
  return json;
}

Json Json::MakeList(List items) {
  Json json;
  json.kind_ = Kind::kList;
  json.list_ = std::move(items);
  return json;
}

Json Json::MakeMap(Map entries) {
  Json json;
  json.kind_ = Kind::kMap;
  for (auto& entry : entries) {
    json.Set(entry.first, std::move(entry.second));
  }
  return json;
}

const Json* Json::Find(const std::string& key) const {
  if (kind_ != Kind::kMap) return nullptr;
  for (const auto& entry : map_) {
    if (entry.first == key) return &entry.second;
  }
  return nullptr;
}

void Json::Set(const std::string& key, Json value) {
  if (kind_ == Kind::kNull) kind_ = Kind::kMap;
  if (kind_ != Kind::kMap) return;
  for (auto& entry : map_) {
    if (entry.first == key) {
      entry.second = std::move(value);
      return;
    }
  }
  map_.emplace_back(key, std::move(value));
}

bool Json::operator==(const Json& other) const {
  if (kind_ != other.kind_) return false;
  switch (kind_) {
    case Kind::kNull:
      return true;
    case Kind::kBool:
      return bool_ == other.bool_;
    case Kind::kInt:
      return int_ == other.int_;
    case Kind::kDouble:
      return double_ == other.double_;
    case Kind::kString:
      return string_ == other.string_;
    case Kind::kList:
      return list_ == other.list_;
    case Kind::kMap: {
      // Order doesn't matter: the same keys with the same values.
      if (map_.size() != other.map_.size()) return false;
      for (const auto& entry : map_) {
        const Json* theirs = other.Find(entry.first);
        if (theirs == nullptr || *theirs != entry.second) return false;
      }
      return true;
    }
  }
  return false;
}

namespace {

void WriteString(const std::string& text, std::string* out) {
  out->push_back('"');
  for (unsigned char c : text) {
    switch (c) {
      case '"':
        out->append("\\\"");
        break;
      case '\\':
        out->append("\\\\");
        break;
      case '\b':
        out->append("\\b");
        break;
      case '\f':
        out->append("\\f");
        break;
      case '\n':
        out->append("\\n");
        break;
      case '\r':
        out->append("\\r");
        break;
      case '\t':
        out->append("\\t");
        break;
      default:
        if (c < 0x20) {
          char escaped[8];
          std::snprintf(escaped, sizeof(escaped), "\\u%04x",
                        static_cast<unsigned int>(c));
          out->append(escaped);
        } else {
          out->push_back(static_cast<char>(c));
        }
    }
  }
  out->push_back('"');
}

void Write(const Json& value, std::string* out) {
  switch (value.kind()) {
    case Json::Kind::kNull:
      out->append("null");
      break;
    case Json::Kind::kBool:
      out->append(value.bool_value() ? "true" : "false");
      break;
    case Json::Kind::kInt:
      out->append(std::to_string(value.int_value()));
      break;
    case Json::Kind::kDouble: {
      double number = value.double_value();
      if (!std::isfinite(number)) {
        out->append("null");
        break;
      }
      char text[32];
      std::snprintf(text, sizeof(text), "%.17g", number);
      out->append(text);
      // Still a double when read back: 2.0 stays 2.0, not 2.
      if (std::strpbrk(text, ".eE") == nullptr) out->append(".0");
      break;
    }
    case Json::Kind::kString:
      WriteString(value.string_value(), out);
      break;
    case Json::Kind::kList: {
      out->push_back('[');
      bool first = true;
      for (const auto& item : value.list()) {
        if (!first) out->push_back(',');
        first = false;
        Write(item, out);
      }
      out->push_back(']');
      break;
    }
    case Json::Kind::kMap: {
      out->push_back('{');
      bool first = true;
      for (const auto& entry : value.map()) {
        if (!first) out->push_back(',');
        first = false;
        WriteString(entry.first, out);
        out->push_back(':');
        Write(entry.second, out);
      }
      out->push_back('}');
      break;
    }
  }
}

constexpr int kMaxDepth = 64;

class Reader {
 public:
  explicit Reader(const std::string& text) : text_(text) {}

  std::optional<Json> ReadDocument() {
    SkipSpace();
    std::optional<Json> value = ReadValue(0);
    if (!value) return std::nullopt;
    SkipSpace();
    if (at_ != text_.size()) return Fail("text after the value");
    return value;
  }

  const std::string& error() const { return error_; }

 private:
  std::optional<Json> Fail(const char* why) {
    if (error_.empty()) {
      error_ = std::string(why) + " at byte " + std::to_string(at_);
    }
    return std::nullopt;
  }

  void SkipSpace() {
    while (at_ < text_.size()) {
      char c = text_[at_];
      if (c != ' ' && c != '\t' && c != '\n' && c != '\r') break;
      ++at_;
    }
  }

  bool Consume(const char* word) {
    size_t length = std::strlen(word);
    if (text_.compare(at_, length, word) != 0) return false;
    at_ += length;
    return true;
  }

  std::optional<Json> ReadValue(int depth) {
    if (depth > kMaxDepth) return Fail("nested too deep");
    if (at_ >= text_.size()) return Fail("unexpected end");
    char c = text_[at_];
    switch (c) {
      case '{':
        return ReadMap(depth);
      case '[':
        return ReadList(depth);
      case '"': {
        std::optional<std::string> text = ReadString();
        if (!text) return std::nullopt;
        return Json::String(std::move(*text));
      }
      case 't':
        if (Consume("true")) return Json::Bool(true);
        return Fail("bad literal");
      case 'f':
        if (Consume("false")) return Json::Bool(false);
        return Fail("bad literal");
      case 'n':
        if (Consume("null")) return Json();
        return Fail("bad literal");
      default:
        if (c == '-' || (c >= '0' && c <= '9')) return ReadNumber();
        return Fail("unexpected character");
    }
  }

  std::optional<Json> ReadMap(int depth) {
    ++at_;  // {
    Json map = Json::MakeMap();
    SkipSpace();
    if (at_ < text_.size() && text_[at_] == '}') {
      ++at_;
      return map;
    }
    for (;;) {
      SkipSpace();
      if (at_ >= text_.size() || text_[at_] != '"') return Fail("expected a key");
      std::optional<std::string> key = ReadString();
      if (!key) return std::nullopt;
      SkipSpace();
      if (at_ >= text_.size() || text_[at_] != ':') return Fail("expected ':'");
      ++at_;
      SkipSpace();
      std::optional<Json> value = ReadValue(depth + 1);
      if (!value) return std::nullopt;
      map.Set(*key, std::move(*value));
      SkipSpace();
      if (at_ >= text_.size()) return Fail("unexpected end");
      if (text_[at_] == ',') {
        ++at_;
        continue;
      }
      if (text_[at_] == '}') {
        ++at_;
        return map;
      }
      return Fail("expected ',' or '}'");
    }
  }

  std::optional<Json> ReadList(int depth) {
    ++at_;  // [
    Json list = Json::MakeList();
    SkipSpace();
    if (at_ < text_.size() && text_[at_] == ']') {
      ++at_;
      return list;
    }
    for (;;) {
      SkipSpace();
      std::optional<Json> value = ReadValue(depth + 1);
      if (!value) return std::nullopt;
      list.list().push_back(std::move(*value));
      SkipSpace();
      if (at_ >= text_.size()) return Fail("unexpected end");
      if (text_[at_] == ',') {
        ++at_;
        continue;
      }
      if (text_[at_] == ']') {
        ++at_;
        return list;
      }
      return Fail("expected ',' or ']'");
    }
  }

  bool ReadHex4(uint32_t* value) {
    if (at_ + 4 > text_.size()) return false;
    uint32_t result = 0;
    for (int i = 0; i < 4; ++i) {
      char c = text_[at_++];
      result <<= 4;
      if (c >= '0' && c <= '9') {
        result |= static_cast<uint32_t>(c - '0');
      } else if (c >= 'a' && c <= 'f') {
        result |= static_cast<uint32_t>(c - 'a' + 10);
      } else if (c >= 'A' && c <= 'F') {
        result |= static_cast<uint32_t>(c - 'A' + 10);
      } else {
        return false;
      }
    }
    *value = result;
    return true;
  }

  static void AppendUtf8(uint32_t code, std::string* out) {
    if (code < 0x80) {
      out->push_back(static_cast<char>(code));
    } else if (code < 0x800) {
      out->push_back(static_cast<char>(0xC0 | (code >> 6)));
      out->push_back(static_cast<char>(0x80 | (code & 0x3F)));
    } else if (code < 0x10000) {
      out->push_back(static_cast<char>(0xE0 | (code >> 12)));
      out->push_back(static_cast<char>(0x80 | ((code >> 6) & 0x3F)));
      out->push_back(static_cast<char>(0x80 | (code & 0x3F)));
    } else {
      out->push_back(static_cast<char>(0xF0 | (code >> 18)));
      out->push_back(static_cast<char>(0x80 | ((code >> 12) & 0x3F)));
      out->push_back(static_cast<char>(0x80 | ((code >> 6) & 0x3F)));
      out->push_back(static_cast<char>(0x80 | (code & 0x3F)));
    }
  }

  std::optional<std::string> ReadString() {
    ++at_;  // "
    std::string out;
    for (;;) {
      if (at_ >= text_.size()) {
        Fail("unterminated string");
        return std::nullopt;
      }
      unsigned char c = static_cast<unsigned char>(text_[at_++]);
      if (c == '"') return out;
      if (c < 0x20) {
        Fail("control character in string");
        return std::nullopt;
      }
      if (c != '\\') {
        out.push_back(static_cast<char>(c));
        continue;
      }
      if (at_ >= text_.size()) {
        Fail("unterminated escape");
        return std::nullopt;
      }
      char escape = text_[at_++];
      switch (escape) {
        case '"':
          out.push_back('"');
          break;
        case '\\':
          out.push_back('\\');
          break;
        case '/':
          out.push_back('/');
          break;
        case 'b':
          out.push_back('\b');
          break;
        case 'f':
          out.push_back('\f');
          break;
        case 'n':
          out.push_back('\n');
          break;
        case 'r':
          out.push_back('\r');
          break;
        case 't':
          out.push_back('\t');
          break;
        case 'u': {
          uint32_t code = 0;
          if (!ReadHex4(&code)) {
            Fail("bad \\u escape");
            return std::nullopt;
          }
          if (code >= 0xD800 && code <= 0xDBFF) {
            // A high surrogate: its low half must follow.
            uint32_t low = 0;
            if (!Consume("\\u") || !ReadHex4(&low) || low < 0xDC00 ||
                low > 0xDFFF) {
              Fail("unpaired surrogate");
              return std::nullopt;
            }
            code = 0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00);
          } else if (code >= 0xDC00 && code <= 0xDFFF) {
            Fail("unpaired surrogate");
            return std::nullopt;
          }
          AppendUtf8(code, &out);
          break;
        }
        default:
          Fail("bad escape");
          return std::nullopt;
      }
    }
  }

  std::optional<Json> ReadNumber() {
    size_t start = at_;
    bool is_double = false;
    if (text_[at_] == '-') ++at_;
    if (at_ >= text_.size()) return Fail("bad number");
    if (text_[at_] == '0') {
      ++at_;
    } else if (text_[at_] >= '1' && text_[at_] <= '9') {
      while (at_ < text_.size() && text_[at_] >= '0' && text_[at_] <= '9') ++at_;
    } else {
      return Fail("bad number");
    }
    if (at_ < text_.size() && text_[at_] == '.') {
      is_double = true;
      ++at_;
      size_t digits = at_;
      while (at_ < text_.size() && text_[at_] >= '0' && text_[at_] <= '9') ++at_;
      if (at_ == digits) return Fail("bad number");
    }
    if (at_ < text_.size() && (text_[at_] == 'e' || text_[at_] == 'E')) {
      is_double = true;
      ++at_;
      if (at_ < text_.size() && (text_[at_] == '+' || text_[at_] == '-')) ++at_;
      size_t digits = at_;
      while (at_ < text_.size() && text_[at_] >= '0' && text_[at_] <= '9') ++at_;
      if (at_ == digits) return Fail("bad number");
    }
    std::string number = text_.substr(start, at_ - start);
    if (!is_double) {
      errno = 0;
      char* end = nullptr;
      long long value = std::strtoll(number.c_str(), &end, 10);
      if (errno == 0 && end != nullptr && *end == '\0') {
        return Json::Int(static_cast<int64_t>(value));
      }
      // Too big for 64 bits: kept as a double, as JSONSerialization would.
    }
    return Json::Double(std::strtod(number.c_str(), nullptr));
  }

  const std::string& text_;
  size_t at_ = 0;
  std::string error_;
};

}  // namespace

std::string WriteJson(const Json& value) {
  std::string out;
  Write(value, &out);
  return out;
}

std::optional<Json> ReadJson(const std::string& text, std::string* error) {
  Reader reader(text);
  std::optional<Json> value = reader.ReadDocument();
  if (!value && error != nullptr) *error = reader.error();
  return value;
}

}  // namespace claktory
