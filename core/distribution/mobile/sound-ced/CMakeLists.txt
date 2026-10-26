cmake_minimum_required(VERSION 3.18)
project(solstone_sound_ced LANGUAGES C CXX)

# Compile the maintained standalone sound-tag source without changing the
# Parakeet dependency's C/C++ symbols. Both engines use the same GGML closure.
if(NOT CED_SOURCE OR NOT GGML_SOURCE)
  message(FATAL_ERROR "CED_SOURCE and GGML_SOURCE are required")
endif()
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
add_subdirectory("${GGML_SOURCE}" ggml)
add_library(solstone_sound_ced STATIC
  "${CED_SOURCE}/src/model_loader.cpp"
  "${CED_SOURCE}/src/ced_runner.cpp"
  "${CED_SOURCE}/src/fft.cpp"
  "${CED_SOURCE}/src/mel.cpp"
  "${CED_SOURCE}/src/audio_io.cpp"
  "${CED_SOURCE}/src/ced.cpp"
  "${CED_SOURCE}/src/ced_capi.cpp")
target_include_directories(solstone_sound_ced PRIVATE
  "${CED_SOURCE}/include" "${CED_SOURCE}/src" "${CED_SOURCE}/third_party")
target_compile_definitions(solstone_sound_ced PRIVATE
  ced=solstone_sound_ced
  DRWAV_API=static
  DRWAV_PRIVATE=static
  ced_capi_abi_version=solstone_sound_ced_capi_abi_version
  ced_capi_load=solstone_sound_ced_capi_load
  ced_capi_free=solstone_sound_ced_capi_free
  ced_capi_last_error=solstone_sound_ced_capi_last_error
  ced_capi_num_classes=solstone_sound_ced_capi_num_classes
  ced_capi_label=solstone_sound_ced_capi_label
  ced_capi_sample_rate=solstone_sound_ced_capi_sample_rate
  ced_capi_classify_pcm_json=solstone_sound_ced_capi_classify_pcm_json
  ced_capi_classify_path_json=solstone_sound_ced_capi_classify_path_json
  ced_capi_classify_pcm=solstone_sound_ced_capi_classify_pcm
  ced_capi_free_string=solstone_sound_ced_capi_free_string)
target_link_libraries(solstone_sound_ced PUBLIC ggml)
