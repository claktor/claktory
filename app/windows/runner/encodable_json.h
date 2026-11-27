#ifndef RUNNER_ENCODABLE_JSON_H_
#define RUNNER_ENCODABLE_JSON_H_

#include <flutter/encodable_value.h>

#include <optional>
#include <string>

#include "core/json.h"

// What a channel hands over (Flutter's standard codec) as JSON, and back.
// Strings, numbers, booleans, null, lists and maps with string keys go
// through; anything else (bytes, a map with other keys) is refused with
// [error] saying what, as the Mac's JSONSerialization refuses them.
std::optional<claktory::Json> JsonFromEncodable(
    const flutter::EncodableValue& value, std::string* error);

flutter::EncodableValue EncodableFromJson(const claktory::Json& json);

#endif  // RUNNER_ENCODABLE_JSON_H_
