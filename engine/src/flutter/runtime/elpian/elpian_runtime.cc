// Copyright 2013 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "flutter/runtime/elpian/elpian_runtime.h"

#include <cstring>

#include "flutter/display_list/dl_builder.h"
#include "flutter/display_list/dl_paint.h"
#include "flutter/fml/logging.h"
#include "flutter/runtime/elpian/elpian_ffi.h"
#include "third_party/rapidjson/include/rapidjson/document.h"

namespace flutter {

ElpianRuntime::ElpianRuntime() = default;
ElpianRuntime::~ElpianRuntime() = default;

bool ElpianRuntime::LoadBundle(const std::string& source) {
  uint8_t* buf = elpian_alloc(source.size());
  std::memcpy(buf, source.data(), source.size());
  const int32_t rc = elpian_init(buf, source.size());
  elpian_free(buf, source.size());
  loaded_ = rc == 0;
  if (!loaded_) {
    FML_LOG(ERROR) << "ElpianRuntime: bundle failed to compile/load";
  }
  return loaded_;
}

bool ElpianRuntime::LoadWidgetApp(const std::string& source) {
  uint8_t* buf = elpian_alloc(source.size());
  std::memcpy(buf, source.data(), source.size());
  const int32_t rc = elpian_init_widgets(buf, source.size());
  elpian_free(buf, source.size());
  loaded_ = rc == 0;
  if (!loaded_) {
    FML_LOG(ERROR) << "ElpianRuntime: widget app failed to compile/load";
  }
  return loaded_;
}

bool ElpianRuntime::LoadFlutterApp(const std::string& source) {
  uint8_t* buf = elpian_alloc(source.size());
  std::memcpy(buf, source.data(), source.size());
  const int32_t rc = elpian_init_flutter(buf, source.size());
  elpian_free(buf, source.size());
  loaded_ = rc == 0;
  if (!loaded_) {
    FML_LOG(ERROR) << "ElpianRuntime: flutter.dart app failed to compile/load";
  }
  return loaded_;
}

void ElpianRuntime::DispatchPointer(double x, double y, bool down) {
  if (loaded_) {
    elpian_pointer(x, y, down ? 1 : 0);
  }
}

namespace {

// ARGB (0xAARRGGBB) integer from the scene tree -> DlColor.
DlColor ColorFromArgb(uint32_t argb) {
  return DlColor(argb);
}

// Lower one `dart:ui` scene op onto the DisplayList builder.
void EmitOp(const rapidjson::Value& op, DisplayListBuilder& builder) {
  if (!op.HasMember("op") || !op["op"].IsString()) {
    return;
  }
  const std::string kind = op["op"].GetString();

  if (kind == "drawRect" && op.HasMember("rect") && op["rect"].IsArray()) {
    const auto& r = op["rect"];
    DlPaint paint;
    paint.setColor(ColorFromArgb(op["color"].GetUint()));
    builder.DrawRect(DlRect::MakeLTRB(r[0].GetDouble(), r[1].GetDouble(),
                                      r[2].GetDouble(), r[3].GetDouble()),
                     paint);
  } else if (kind == "drawCircle" && op.HasMember("center")) {
    const auto& c = op["center"];
    DlPaint paint;
    paint.setColor(ColorFromArgb(op["color"].GetUint()));
    builder.DrawCircle(DlPoint(c[0].GetDouble(), c[1].GetDouble()),
                       op["radius"].GetDouble(), paint);
  }
  // NOTE: drawParagraph maps to DlBuilder::DrawText, which needs a laid-out
  // text frame (ParagraphBuilder/text layout) — wired in a follow-up; the scene
  // contract already carries text/offset/fontSize/color.
}

}  // namespace

sk_sp<DisplayList> ElpianRuntime::RenderFrame() {
  if (!loaded_) {
    return nullptr;
  }
  const size_t len = elpian_frame();
  const uint8_t* ptr = elpian_result_ptr();
  const std::string json(reinterpret_cast<const char*>(ptr), len);

  rapidjson::Document doc;
  doc.Parse(json.c_str());
  if (doc.HasParseError() || doc.HasMember("error") || !doc.HasMember("root")) {
    return nullptr;
  }
  const auto& root = doc["root"];
  if (!root.HasMember("ops") || !root["ops"].IsArray()) {
    return nullptr;
  }

  DisplayListBuilder builder;
  for (const auto& op : root["ops"].GetArray()) {
    EmitOp(op, builder);
  }
  return builder.Build();
}

}  // namespace flutter
