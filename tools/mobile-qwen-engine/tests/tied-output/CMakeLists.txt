# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
cmake_minimum_required(VERSION 3.18)
project(qwen_tied_output_control LANGUAGES CXX)
# Supply an actual pinned llama archive and its matching GGML family. This
# standalone control does not rebuild a toolchain, engine or native family.
foreach(input LLAMA_SOURCE LLAMA_LIBRARY GGML_BUILD)
    if(NOT DEFINED ${input})
        message(FATAL_ERROR "Explicit ${input} is required")
    endif()
endforeach()
find_package(Threads REQUIRED)
find_package(OpenMP)
add_executable(qwen_tied_output_control control.cpp)
target_compile_features(qwen_tied_output_control PRIVATE cxx_std_17)
target_compile_definitions(qwen_tied_output_control PRIVATE NDEBUG)
target_include_directories(qwen_tied_output_control PRIVATE
    "${LLAMA_SOURCE}/include" "${LLAMA_SOURCE}/src" "${LLAMA_SOURCE}/ggml/include")
target_link_libraries(qwen_tied_output_control PRIVATE "${LLAMA_LIBRARY}"
    "${GGML_BUILD}/libggml.a" "${GGML_BUILD}/libggml-cpu.a"
    "${GGML_BUILD}/libggml-base.a" Threads::Threads ${CMAKE_DL_LIBS})
if(OpenMP_CXX_FOUND)
    target_link_libraries(qwen_tied_output_control PRIVATE OpenMP::OpenMP_CXX)
endif()
if(UNIX)
    target_link_libraries(qwen_tied_output_control PRIVATE m)
endif()
enable_testing()
add_test(NAME qwen_tied_output_control COMMAND qwen_tied_output_control)
set_tests_properties(qwen_tied_output_control PROPERTIES TIMEOUT 20)
