# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
cmake_minimum_required(VERSION 3.22.1)
project(SolstoneVulkanLoader LANGUAGES C)
add_subdirectory("../loader" upstream)
foreach(target vulkan asm_offset)
    if(NOT TARGET ${target})
        message(FATAL_ERROR "Required loader target absent: ${target}")
    endif()
    set_property(TARGET ${target} PROPERTY MSVC_RUNTIME_LIBRARY "MultiThreadedDLL")
endforeach()
