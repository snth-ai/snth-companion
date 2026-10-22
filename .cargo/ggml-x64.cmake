# Read by cmake before every project() of whisper.cpp (whisper-rs-sys),
# through CMAKE_PROJECT_INCLUDE_BEFORE in .cargo/config.toml.
#
# On a Windows host: ggml's CPU code for x86-64 with AVX2 (Haswell, Zen
# and later; FMA and F16C come with AVX2 under MSVC) and BMI2, never
# tuned for the build machine (GGML_NATIVE would detect the CI runner's
# AVX-512 and ship it). The app checks AVX2, FMA, F16C and BMI2 at run
# time before it offers wake by name (island/wake/mod.rs `available`).
#
# Anywhere else: nothing (the Mac's build stays as it was).
if(CMAKE_HOST_WIN32 OR CMAKE_HOST_SYSTEM_NAME STREQUAL "Windows" OR "$ENV{OS}" STREQUAL "Windows_NT")
  message(STATUS "snth: ggml CPU floor AVX2 (.cargo/ggml-x64.cmake)")
  set(GGML_NATIVE OFF CACHE BOOL "snth: no build-machine tuning" FORCE)
  set(GGML_SSE42 ON CACHE BOOL "snth: x86-64 floor" FORCE)
  set(GGML_AVX ON CACHE BOOL "snth: x86-64 floor" FORCE)
  set(GGML_AVX2 ON CACHE BOOL "snth: x86-64 floor" FORCE)
  set(GGML_BMI2 ON CACHE BOOL "snth: x86-64 floor" FORCE)
  set(GGML_AVX512 OFF CACHE BOOL "snth: x86-64 floor" FORCE)
  set(GGML_AVX512_VBMI OFF CACHE BOOL "snth: x86-64 floor" FORCE)
  set(GGML_AVX512_VNNI OFF CACHE BOOL "snth: x86-64 floor" FORCE)
  set(GGML_AVX512_BF16 OFF CACHE BOOL "snth: x86-64 floor" FORCE)
  set(GGML_AVX_VNNI OFF CACHE BOOL "snth: x86-64 floor" FORCE)
  # A mark the CI looks for: this file was read for this build.
  file(WRITE "${CMAKE_BINARY_DIR}/snth-ggml-floor.txt" "AVX2\n")
endif()
