cmake_minimum_required(VERSION 3.18)
project(cognos_lstm_predictor CXX)
set(CMAKE_CXX_STANDARD 17)

add_compile_options(-O2 -Wall -Wextra)

# Find ONNX Runtime — set ONNXRUNTIME_ROOT if not in system path
find_path(ONNXRUNTIME_INCLUDE_DIR
    NAMES onnxruntime/core/session/onnxruntime_cxx_api.h
    PATHS ${ONNXRUNTIME_ROOT}/include /usr/include /usr/local/include)

find_library(ONNXRUNTIME_LIB
    NAMES onnxruntime
    PATHS ${ONNXRUNTIME_ROOT}/lib /usr/lib /usr/local/lib)

if(NOT ONNXRUNTIME_INCLUDE_DIR OR NOT ONNXRUNTIME_LIB)
    message(FATAL_ERROR
        "ONNX Runtime not found. Set -DONNXRUNTIME_ROOT=/path/to/onnxruntime")
endif()

add_executable(cognos-lstm-predictor
    lstm_predictor.cpp
    lstm_predictor.h)

target_include_directories(cognos-lstm-predictor PRIVATE ${ONNXRUNTIME_INCLUDE_DIR})
target_link_libraries(cognos-lstm-predictor ${ONNXRUNTIME_LIB})

set_target_properties(cognos-lstm-predictor PROPERTIES
    RUNTIME_OUTPUT_DIRECTORY ${CMAKE_SOURCE_DIR}/../../../build/output/bin)