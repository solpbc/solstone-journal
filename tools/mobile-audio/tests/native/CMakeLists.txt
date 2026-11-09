# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
cmake_minimum_required(VERSION 3.18)
project(parakeet_graph_failure_control LANGUAGES C CXX)

if(NOT EXISTS "${PARAKEET_SOURCE}/src/ggml_graph.cpp" OR
   NOT EXISTS "${GGML_SOURCE}/CMakeLists.txt")
    message(FATAL_ERROR "Explicit pinned Parakeet and GGML sources are required")
endif()
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_BUILD_TYPE Release)
set(GGML_NATIVE OFF CACHE BOOL "" FORCE)
set(GGML_OPENMP OFF CACHE BOOL "" FORCE)
set(GGML_LLAMAFILE OFF CACHE BOOL "" FORCE)
set(GGML_BACKEND_DL OFF CACHE BOOL "" FORCE)
set(GGML_METAL OFF CACHE BOOL "" FORCE)
set(GGML_CUDA OFF CACHE BOOL "" FORCE)
set(GGML_VULKAN OFF CACHE BOOL "" FORCE)
set(GGML_BLAS OFF CACHE BOOL "" FORCE)
set(GGML_BUILD_TESTS OFF CACHE BOOL "" FORCE)
set(BUILD_SHARED_LIBS OFF CACHE BOOL "" FORCE)
add_subdirectory("${GGML_SOURCE}" ggml)
find_package(Threads REQUIRED)
add_library(parakeet_graph_control_backend STATIC
    "${PARAKEET_SOURCE}/src/backend.cpp"
    "${PARAKEET_SOURCE}/src/backend_pool.cpp")
target_include_directories(parakeet_graph_control_backend PUBLIC
    "${PARAKEET_SOURCE}/src" "${PARAKEET_SOURCE}/include")
target_compile_definitions(parakeet_graph_control_backend PUBLIC NDEBUG)
# The backend's unused model-weight helpers are outside this no-model control.
target_compile_options(parakeet_graph_control_backend PRIVATE -ffunction-sections -fdata-sections)
target_link_libraries(parakeet_graph_control_backend PUBLIC ggml Threads::Threads)
function(add_graph_control target graph_source)
    add_executable(${target} graph_failure.cpp "${graph_source}")
    target_link_libraries(${target} PRIVATE parakeet_graph_control_backend)
    if(APPLE)
        target_link_options(${target} PRIVATE -Wl,-dead_strip)
    else()
        target_link_options(${target} PRIVATE -Wl,--gc-sections)
    endif()
endfunction()
add_graph_control(parakeet_graph_failure_control "${PARAKEET_SOURCE}/src/ggml_graph.cpp")
# Optional retained unpatched source gives an honest old-behavior negative,
# without rebuilding GGML or introducing a production failure switch.
if(EXISTS "${PARAKEET_BASELINE_GRAPH_SOURCE}")
    add_graph_control(parakeet_graph_failure_baseline "${PARAKEET_BASELINE_GRAPH_SOURCE}")
endif()
enable_testing()
add_test(NAME parakeet_graph_failure_control COMMAND parakeet_graph_failure_control)
set_tests_properties(parakeet_graph_failure_control PROPERTIES TIMEOUT 15 ENVIRONMENT "PARAKEET_DEVICE=cpu")
