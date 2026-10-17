# CwlCatchException 2.2.1 (github.com/mattgallagher/CwlCatchException), the
# Swift package speech_to_text's Package.swift depends on, as its two
# SwiftPM targets.

load("@rules_cc//cc:objc_library.bzl", "objc_library")
load("@rules_swift//swift:swift_library.bzl", "swift_library")

objc_library(
    name = "CwlCatchExceptionSupport",
    srcs = ["Sources/CwlCatchExceptionSupport/CwlCatchException.m"],
    hdrs = ["Sources/CwlCatchExceptionSupport/include/CwlCatchException.h"],
    includes = ["Sources/CwlCatchExceptionSupport/include"],
    module_name = "CwlCatchExceptionSupport",
    sdk_frameworks = ["Foundation"],
)

swift_library(
    name = "CwlCatchException",
    srcs = ["Sources/CwlCatchException/CwlCatchException.swift"],
    # As SwiftPM builds it: the Swift target imports its Objective-C one.
    defines = ["SWIFT_PACKAGE"],
    module_name = "CwlCatchException",
    visibility = ["//visibility:public"],
    deps = [":CwlCatchExceptionSupport"],
)
