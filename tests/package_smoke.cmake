foreach(required SDHLT_BUILD_DIR SDHLT_INSTALL_DIR SDHLT_PYTHON SDHLT_SOURCE_DIR)
    if (NOT DEFINED ${required} OR "${${required}}" STREQUAL "")
        message(FATAL_ERROR "package smoke is missing -D${required}=...")
    endif()
endforeach()

# This directory is fixed below the active build tree and contains only output
# from this test. Starting clean prevents a stale executable from hiding a
# missing install rule.
get_filename_component(build_root "${SDHLT_BUILD_DIR}" ABSOLUTE)
get_filename_component(install_root "${SDHLT_INSTALL_DIR}" ABSOLUTE)
file(TO_CMAKE_PATH "${build_root}/" build_prefix)
file(TO_CMAKE_PATH "${install_root}/" install_prefix)
string(FIND "${install_prefix}" "${build_prefix}" install_is_below_build)
if (NOT install_is_below_build EQUAL 0 OR install_prefix STREQUAL build_prefix)
    message(FATAL_ERROR "refusing to clean package smoke path outside the build tree: ${install_root}")
endif()
file(REMOVE_RECURSE "${install_root}")

set(install_command
    "${CMAKE_COMMAND}" --install "${SDHLT_BUILD_DIR}"
    --prefix "${install_root}")
if (DEFINED SDHLT_CONFIG AND NOT "${SDHLT_CONFIG}" STREQUAL "")
    list(APPEND install_command --config "${SDHLT_CONFIG}")
endif()

execute_process(
    COMMAND ${install_command}
    RESULT_VARIABLE install_result
    COMMAND_ECHO STDOUT)
if (NOT install_result EQUAL 0)
    message(FATAL_ERROR "cmake --install failed with exit code ${install_result}")
endif()

execute_process(
    COMMAND "${SDHLT_PYTHON}" -B
        "${SDHLT_SOURCE_DIR}/tests/smoke_package.py"
        --tools-dir "${install_root}"
    WORKING_DIRECTORY "${SDHLT_SOURCE_DIR}"
    RESULT_VARIABLE smoke_result
    COMMAND_ECHO STDOUT)
if (NOT smoke_result EQUAL 0)
    message(FATAL_ERROR "packaged tool smoke failed with exit code ${smoke_result}")
endif()
