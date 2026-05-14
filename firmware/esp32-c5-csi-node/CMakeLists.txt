# ESP32 CSI Node Firmware (ADR-018)
# ⚠️ ESP-IDF v5.5+ REQUIRED for ESP32-C5 (v5.4 has C5 CSI 5GHz cache bug)
cmake_minimum_required(VERSION 3.16)

set(EXTRA_COMPONENT_DIRS "")

include($ENV{IDF_PATH}/tools/cmake/project.cmake)

# Enforce ESP-IDF v5.5+ for C5 target to avoid known CSI cache bug
if(CONFIG_IDF_TARGET_ESP32C5 OR CONFIG_IDF_TARGET_ESP32C6 OR CONFIG_IDF_TARGET_ESP32C61)
    if(IDF_VERSION_MAJOR LESS 5 OR (IDF_VERSION_MAJOR EQUAL 5 AND IDF_VERSION_MINOR LESS 5))
        message(FATAL_ERROR "ESP-IDF v5.5+ required for ${CONFIG_IDF_TARGET}. "
                "Current: ${IDF_VERSION_MAJOR}.${IDF_VERSION_MINOR}. "
                "v5.4 has a known CSI cache bug on 5GHz (IDFGH-17558). "
                "Please upgrade to v5.5+ before building.")
    endif()
endif()

project(esp32-csi-node)
