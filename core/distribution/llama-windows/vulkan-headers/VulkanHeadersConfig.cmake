# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
if(NOT DEFINED ENV{VULKAN_SDK})
    message(FATAL_ERROR "Verified Vulkan SDK root is required")
endif()
file(READ "$ENV{VULKAN_SDK}/Include/vulkan/vulkan_core.h" header)
if(NOT header MATCHES "#define VK_HEADER_VERSION 357[\r\n]")
    message(FATAL_ERROR "Vulkan SDK header version differs from the reviewed input")
endif()
set(VulkanHeaders_VERSION "1.4.357")
add_library(Vulkan::Headers INTERFACE IMPORTED GLOBAL)
set_target_properties(Vulkan::Headers PROPERTIES
    INTERFACE_INCLUDE_DIRECTORIES "$ENV{VULKAN_SDK}/Include")
