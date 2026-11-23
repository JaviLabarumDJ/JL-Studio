// SPDX-License-Identifier: GPL-3.0-or-later
#if defined(_WIN32)
 #ifndef NOMINMAX
  #define NOMINMAX
 #endif
 #include <windows.h>
#else
 #include <dlfcn.h>
#endif

namespace mlstock {
Present presenter() {
#if defined(_WIN32)
    static const auto fn = reinterpret_cast<Present>(GetProcAddress(GetModuleHandleW(nullptr), "ml_stock_editor_pixels_v1"));
#else
    static const auto fn = reinterpret_cast<Present>(dlsym(RTLD_DEFAULT, "ml_stock_editor_pixels_v1"));
#endif
    return fn;
}
}
