// Copyright (c) vladkens | MIT License | https://github.com/vladkens/cargo-solo
//! The sysroots: each target's packages from the distros, rustup, Zig, LLVM and crates.io, pinned
//! by SHA-256, fetched by the system's `curl` on first use and unpacked into the cache.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{env, fs, io};

use crate::unpack::{copy, cpio, gunzip, materialize, member, payload, unpack, xz};
use crate::{bad, run};

/// Bumped when unpacking changes, so that every sysroot is unpacked anew into another folder.
const FORMAT: u32 = 2;

// MARK: Packages

/// The C library of a sysroot, which decides the flags clang gets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Libc {
  Glibc,
  Musl,
  Mingw,
  Darwin,
}

impl Libc {
  /// The library and its version, for the help.
  pub fn version(self) -> &'static str {
    match self {
      Libc::Glibc => "glibc 2.28",
      Libc::Musl => "musl 1.2.5",
      Libc::Mingw => "mingw-w64 12.0",
      Libc::Darwin => "libSystem, SDK 27.0, libc++ 20",
    }
  }
}

/// A file of a distro, rustup, Zig or LLVM, tried from each mirror in turn: the SHA-256 decides.
#[derive(Debug)]
pub struct Package {
  pub mirrors: &'static [&'static str],
  /// The path under each mirror, of a `.deb`, an `.rpm`, an `.apk`, a `.crate`, a `.tar.xz` or a
  /// `.tar.gz`.
  pub path: &'static str,
  pub sha256: &'static str,
  /// The folder in the archive the files are taken from; of those, the ones whose path there
  /// starts with one of `keep` go under `into` in the sysroot.
  pub root: &'static str,
  pub keep: &'static [&'static str],
  pub into: &'static str,
}

/// What a sysroot keeps of a Debian package: the headers and the libraries, and the folder of
/// mingw-w64's headers, which its include folder links into.
const DEB_KEEP: &[&str] = &[
  "lib/",
  "lib64/",
  "usr/include/",
  "usr/lib/",
  "usr/share/mingw-w64/",
  "usr/x86_64-w64-mingw32/",
];

/// Debian 10 buster, frozen on archive.debian.org since 2024-03-31, and snapshot.debian.org's copy.
const BUSTER: &[&str] = &[
  "https://archive.debian.org/debian",
  "https://snapshot.debian.org/archive/debian-archive/20240331T102506Z/debian",
];

/// Debian 13 trixie, and snapshot.debian.org as of when every package here was current: one that a
/// point release replaces stays there.
const TRIXIE: &[&str] =
  &["https://deb.debian.org/debian", "https://snapshot.debian.org/archive/debian/20260510T204219Z"];

const RUSTUP: &[&str] = &["https://static.rust-lang.org"];

/// Zig's releases on ziglang.org, and a community mirror Zig lists, which serves the same files.
const ZIG: &[&str] = &["https://ziglang.org/download/0.17.1", "https://pkg.hexops.org/zig"];

/// LLVM 20.1.8's release on GitHub, the sources of each of its projects.
const LLVM: &[&str] = &["https://github.com/llvm/llvm-project/releases/download/llvmorg-20.1.8"];

/// crates.io's files, which never change once published.
const CRATES_IO: &[&str] = &["https://static.crates.io/crates"];

/// The releases of Swift's CoreFoundation, Apple's own for Swift on Linux and Windows, on GitHub.
const SWIFT_FOUNDATION: &[&str] =
  &["https://github.com/swiftlang/swift-corelibs-foundation/archive/refs/tags"];

/// The releases of configd, SystemConfiguration's source, on GitHub, where Apple publishes it.
const CONFIGD: &[&str] = &["https://github.com/apple-oss-distributions/configd/archive/refs/tags"];

/// The releases of Security, its framework's source, on GitHub, where Apple publishes it.
const SECURITY: &[&str] =
  &["https://github.com/apple-oss-distributions/Security/archive/refs/tags"];

/// AlmaLinux 8.10's AppStream, RHEL 8's rebuilt, and kernel.org's copy: 8's last release, whose
/// packages stay until 8's end in 2029.
const ALMA8: &[&str] = &[
  "https://repo.almalinux.org/almalinux/8.10/AppStream",
  "https://mirrors.edge.kernel.org/almalinux/8.10/AppStream",
];

/// Alpine 3.21's main, and kernel.org's copy: out of support since 2026-11-01, so its packages no
/// longer change.
const ALPINE: &[&str] = &[
  "https://dl-cdn.alpinelinux.org/alpine/v3.21/main",
  "https://mirrors.edge.kernel.org/alpine/v3.21/main",
];

impl Package {
  const fn deb(
    mirrors: &'static [&'static str],
    path: &'static str,
    sha256: &'static str,
  ) -> Package {
    Package { mirrors, path, sha256, root: "", keep: DEB_KEEP, into: "" }
  }

  const fn zig(root: &'static str, keep: &'static [&'static str], into: &'static str) -> Package {
    Package {
      mirrors: ZIG,
      path: "zig-0.17.1.tar.xz",
      sha256: "1e89bf06610b769a8c77676c6c8d54b093a0db60f4891f2f91014829012f0e24",
      root,
      keep,
      into,
    }
  }

  /// A crate of objc2's that binds an Apple framework, or objc2's own, unpacked into [OBJC2] for
  /// [sdk].
  const fn objc2(path: &'static str, sha256: &'static str) -> Package {
    Package { mirrors: CRATES_IO, path, sha256, root: "", keep: &[""], into: OBJC2 }
  }

  const fn only(
    self,
    root: &'static str,
    keep: &'static [&'static str],
    into: &'static str,
  ) -> Package {
    Package { root, keep, into, ..self }
  }

  /// Where `path`, a file of its archive, goes in the sysroot; `None` when it's not kept.
  pub fn place(&self, path: &str) -> Option<PathBuf> {
    let rest = path.strip_prefix(self.root)?;
    // glibc's charset modules, loaded at run time only: 7 of its 12 MB; libc++'s frozen C++03
    // headers, which only a define of the user's takes: 9 of its 19 MB.
    let unused = rest.contains("/gconv/") || rest.starts_with("__cxx03/");
    if !self.keep.iter().any(|x| rest.starts_with(x)) || unused {
      return None;
    }

    Some(PathBuf::from(format!("{}{rest}", self.into)))
  }
}

/// The kernel's headers, Linux 4.19's, for x86_64: glibc's and musl's.
const LINUX_X64: Package = Package::deb(
  BUSTER,
  "pool/main/l/linux/linux-libc-dev_4.19.249-2_amd64.deb",
  "c512f30f5abe413464b79ed23b54958df8cc104446c3ae93d83d28e23728a571",
);

/// [LINUX_X64] for aarch64.
const LINUX_ARM64: Package = Package::deb(
  BUSTER,
  "pool/main/l/linux/linux-libc-dev_4.19.249-2_arm64.deb",
  "e9a92d71d62655f673433c345a03fecaa4292f93c6572b28fd3d16dcfd1cc7f6",
);

/// GCC 8's crtbegin.o and libgcc for x86_64: glibc's and musl's.
const LIBGCC_X64: Package = Package::deb(
  BUSTER,
  "pool/main/g/gcc-8/libgcc-8-dev_8.3.0-6_amd64.deb",
  "a52d5216858c7185bc245e52ad95a6ad515d087bc25e8556df06c5626b41a837",
);

/// [LIBGCC_X64] for aarch64.
const LIBGCC_ARM64: Package = Package::deb(
  BUSTER,
  "pool/main/g/gcc-8/libgcc-8-dev_8.3.0-6_arm64.deb",
  "d8385c0106d4ac559ada75b162aecb9d507a67da3ac8742eb32258f501d07c7e",
);

/// What a musl sysroot takes of [LIBGCC_X64]: libgcc static, with no glibc's libgcc_s.
const MUSL_LIBGCC: &[&str] = &["crtbegin", "crtend", "libgcc.a", "libgcc_eh.a"];

/// glibc 2.28 for x86_64: libc, the kernel's headers, and GCC's crtbegin.o, libgcc and libgcc_s.
pub const GLIBC_X64: &[Package] = &[
  Package::deb(
    BUSTER,
    "pool/main/g/glibc/libc6_2.28-10+deb10u1_amd64.deb",
    "0ee0f0bfcd7b59ffda1dbc76ef58fa79512913204f2a336206467bfbf45bc69f",
  ),
  Package::deb(
    BUSTER,
    "pool/main/g/glibc/libc6-dev_2.28-10+deb10u1_amd64.deb",
    "76b8413920d91a9a4289f99839933d0d30095e0ec12b05bb4f1a7f27e5ef4a55",
  ),
  LINUX_X64,
  LIBGCC_X64,
  Package::deb(
    BUSTER,
    "pool/main/g/gcc-8/libgcc1_8.3.0-6_amd64.deb",
    "b1bb7611f3372732889d502cb1d09fe572b5fbb5288a4a8b1ed0363fecc3555a",
  ),
];

/// [GLIBC_X64] for aarch64.
pub const GLIBC_ARM64: &[Package] = &[
  Package::deb(
    BUSTER,
    "pool/main/g/glibc/libc6_2.28-10+deb10u1_arm64.deb",
    "09f5e92b1527cdc06b757499bb4d97ddf1b064fc3d163f978e5de18b131cd330",
  ),
  Package::deb(
    BUSTER,
    "pool/main/g/glibc/libc6-dev_2.28-10+deb10u1_arm64.deb",
    "04097716018288cef8b82339faa1a9847838b89f39574b5b169f4657da8bf269",
  ),
  LINUX_ARM64,
  LIBGCC_ARM64,
  Package::deb(
    BUSTER,
    "pool/main/g/gcc-8/libgcc1_8.3.0-6_arm64.deb",
    "2851ac25d12958586c035de5ec4f2fc17272dec48f776dd0dd24c62f62674fd9",
  ),
];

/// musl 1.2.5 for x86_64, the one Rust bundles, Debian's in trixie: its headers, and its libc.a and
/// crt objects for what clang links itself, as a configure script's test (Rust links its own,
/// `self-contained`); the kernel's headers; GCC's crtbegin.o and libgcc, in musl's folder.
pub const MUSL_X64: &[Package] = &[
  Package::deb(
    TRIXIE,
    "pool/main/m/musl/musl-dev_1.2.5-3.1~deb13u1_amd64.deb",
    "a3f7d47e452129bb65aae201d6c115abf4d5d3efb136b38b7c6b4fa03ec065e9",
  ),
  LINUX_X64,
  LIBGCC_X64.only("usr/lib/gcc/x86_64-linux-gnu/8/", MUSL_LIBGCC, "usr/lib/x86_64-linux-musl/"),
];

/// [MUSL_X64] for aarch64.
pub const MUSL_ARM64: &[Package] = &[
  Package::deb(
    TRIXIE,
    "pool/main/m/musl/musl-dev_1.2.5-3.1~deb13u1_arm64.deb",
    "384ea029e3694baac696d3993d93246f9a4b3741c493697e1ceb7256d8820748",
  ),
  LINUX_ARM64,
  LIBGCC_ARM64.only("usr/lib/gcc/aarch64-linux-gnu/8/", MUSL_LIBGCC, "usr/lib/aarch64-linux-musl/"),
];

/// mingw-w64 for x86_64: the headers, the CRT with its crtbegin.o, the import libraries; and
/// libgcc, which Debian has only in its 38 MB cross GCC, from rustup's rust-mingw: what rustc links
/// windows-gnu with on a Windows host.
pub const MINGW_X64: &[Package] = &[
  Package::deb(
    TRIXIE,
    "pool/main/m/mingw-w64/mingw-w64-common_12.0.0-5_all.deb",
    "97dce5d0d8aff1cade4786ad4ef5078345477a598829afcac2e7f2fbc5399990",
  ),
  Package::deb(
    TRIXIE,
    "pool/main/m/mingw-w64/mingw-w64-x86-64-dev_12.0.0-5_all.deb",
    "0bf89cf7454cccb49cd2a46a6f7b33896f6ae3a1a2fe33e02c40be6155bbb385",
  ),
  Package {
    mirrors: RUSTUP,
    path: "dist/2026-11-12/rust-mingw-1.100.0-x86_64-pc-windows-gnu.tar.xz",
    sha256: "5491d2ad03b0f124307c8b2c4ac840b65e738a348f4381975f0e638b012e4ae9",
    root: "rust-mingw-1.100.0-x86_64-pc-windows-gnu/rust-mingw/lib/rustlib/x86_64-pc-windows-gnu/lib/self-contained/",
    keep: &["libgcc"],
    into: "usr/x86_64-w64-mingw32/lib/",
  },
];

/// Apple's SDK for both arches, laid out as Xcode's: from Zig's source, macOS's headers and
/// libSystem's `.tbd`, the stub a link needs, not Zig's `SDKSettings.json`, which clang warns of;
/// from LLVM's, libc++ 20's (Zig's 22 needs clang 20) headers and its exports, for [sdk]'s `.tbd`.
/// From objc2's crates, Rust's bindings, what libobjc and the frameworks common crates link
/// export, for [sdk].
pub const DARWIN: &[Package] = &[
  Package::zig("zig-0.17.1/lib/libc/include/any-darwin-any/", &[""], "usr/include/"),
  Package::zig("zig-0.17.1/lib/libc/darwin/", &["libSystem.tbd"], "usr/lib/"),
  LIBCXX.only("libcxx-20.1.8.src/include/", &[""], LIBCXX_INCLUDE),
  LIBCXX.only("libcxx-20.1.8.src/vendor/llvm/", &[ASSERTION_HANDLER], LIBCXX_INCLUDE),
  LIBCXX.only("libcxx-20.1.8.src/lib/abi/", &[ABI_LISTS[0].1, ABI_LISTS[1].1], "usr/lib/"),
  LIBCXXABI.only(
    "libcxxabi-20.1.8.src/include/",
    &["__cxxabi_config.h", "cxxabi.h"],
    LIBCXX_INCLUDE,
  ),
  Package::objc2(
    "objc2/objc2-0.6.3.crate",
    "b7c2599ce0ec54857b29ce62166b0ed9b4f6f1a70ccc9a71165b6154caca8c05",
  ),
  Package::objc2(
    "objc2-core-foundation/objc2-core-foundation-0.3.2.crate",
    "2a180dd8642fa45cdb7dd721cd4c11b1cadd4929ce112ebd8b9f5803cc79d536",
  ),
  Package::objc2(
    "objc2-core-services/objc2-core-services-0.3.2.crate",
    "583300ad934cba24ff5292aee751ecc070f7ca6b39a574cc21b7b5e588e06a0b",
  ),
  Package::objc2(
    "objc2-foundation/objc2-foundation-0.3.2.crate",
    "e3e0adef53c21f888deb4fa59fc59f7eb17404926ee8a6f59f5df0fd7f9f3272",
  ),
  Package::objc2(
    "objc2-io-kit/objc2-io-kit-0.3.2.crate",
    "33fafba39597d6dc1fb709123dfa8289d39406734be322956a69f0931c73bb15",
  ),
  Package::objc2(
    "objc2-open-directory/objc2-open-directory-0.3.2.crate",
    "bb82bed227edf5201dfedf072bba4015a33d3d4a98519837295a90f0a23f676d",
  ),
  Package::objc2(
    "objc2-security/objc2-security-0.3.2.crate",
    "709fe137109bd1e8b5a99390f77a7d8b2961dafc1a1c5db8f2e60329ad6d895a",
  ),
  Package::objc2(
    "objc2-system-configuration/objc2-system-configuration-0.3.2.crate",
    "7216bd11cbda54ccabcab84d523dc93b858ec75ecfb3a7d89513fa22464da396",
  ),
  // From Apple's open source, the headers C takes, set up for macOS by [sdk]: CoreFoundation's
  // from Swift's (Apache 2.0), SystemConfiguration's from configd and Security's (APSL 2.0).
  Package {
    mirrors: SWIFT_FOUNDATION,
    path: "swift-6.4.3-RELEASE.tar.gz",
    sha256: "5dbff647b941ce93884535f5d90620a6dd49c1a7dc62e88f67e4843a26354602",
    root: "swift-corelibs-foundation-swift-6.4.3-RELEASE/Sources/CoreFoundation/include/",
    // Not Swift's own headers: ForSwiftFoundationOnly.h and the module maps.
    keep: &["CF", "CoreFoundation.h"],
    into: "System/Library/Frameworks/CoreFoundation.framework/Headers/",
  },
  Package {
    mirrors: CONFIGD,
    path: "configd-1405.120.5.tar.gz",
    sha256: "f7ca6af8e6eb53a8daccac12d876d3967e62499e1ab5de46f9515487ec7ebe3e",
    root: "configd-configd-1405.120.5/SystemConfiguration.fproj/",
    keep: SC_HEADERS,
    into: "System/Library/Frameworks/SystemConfiguration.framework/Headers/",
  },
  // Its headers for all of Apple's OSs, then macOS's own: in Xcode's SDK, one folder.
  Package {
    mirrors: SECURITY,
    path: "Security-61901.120.67.tar.gz",
    sha256: "aceecd4b51185e22641dfb20438a9094062b92bd64bc3da9d276f329c479dad2",
    root: "Security-Security-61901.120.67/header_symlinks/Security/",
    keep: SECURITY_HEADERS,
    into: "System/Library/Frameworks/Security.framework/Headers/",
  },
  Package {
    mirrors: SECURITY,
    path: "Security-61901.120.67.tar.gz",
    sha256: "aceecd4b51185e22641dfb20438a9094062b92bd64bc3da9d276f329c479dad2",
    root: "Security-Security-61901.120.67/header_symlinks/macOS/Security/",
    keep: SECURITY_HEADERS,
    into: "System/Library/Frameworks/Security.framework/Headers/",
  },
];

/// SystemConfiguration's public headers, the ones Xcode's SDK has of configd's.
const SC_HEADERS: &[&str] = &[
  "CaptiveNetwork.h",
  "DHCPClientPreferences.h",
  "SCDynamicStore.h",
  "SCDynamicStoreCopyDHCPInfo.h",
  "SCDynamicStoreCopySpecific.h",
  "SCDynamicStoreKey.h",
  "SCNetwork.h",
  "SCNetworkConfiguration.h",
  "SCNetworkConnection.h",
  "SCNetworkReachability.h",
  "SCPreferences.h",
  "SCPreferencesPath.h",
  "SCPreferencesSetSpecific.h",
  "SCSchemaDefinitions.h",
  "SystemConfiguration.h",
];

/// Security's public headers, the ones Xcode's SDK has.
const SECURITY_HEADERS: &[&str] = &[
  "Authorization.h",
  "AuthorizationDB.h",
  "AuthorizationPlugin.h",
  "AuthorizationTags.h",
  "AuthSession.h",
  "certextensions.h",
  "CipherSuite.h",
  "CMSDecoder.h",
  "CMSEncoder.h",
  "CodeSigning.h",
  "CSCommon.h",
  "cssm.h",
  "cssmaci.h",
  "cssmapi.h",
  "cssmapple.h",
  "cssmcli.h",
  "cssmconfig.h",
  "cssmcspi.h",
  "cssmdli.h",
  "cssmerr.h",
  "cssmkrapi.h",
  "cssmkrspi.h",
  "cssmspi.h",
  "cssmtpi.h",
  "cssmtype.h",
  "eisl.h",
  "emmspi.h",
  "emmtype.h",
  "mds_schema.h",
  "mds.h",
  "oids.h",
  "oidsalg.h",
  "oidsattr.h",
  "oidsbase.h",
  "oidscert.h",
  "oidscrl.h",
  "SecAccess.h",
  "SecAccessControl.h",
  "SecACL.h",
  "SecAsn1Coder.h",
  "SecAsn1Templates.h",
  "SecAsn1Types.h",
  "SecBase.h",
  "SecCertificate.h",
  "SecCertificateOIDs.h",
  "SecCode.h",
  "SecCodeHost.h",
  "SecCustomTransform.h",
  "SecDecodeTransform.h",
  "SecDigestTransform.h",
  "SecEncodeTransform.h",
  "SecEncryptTransform.h",
  "SecIdentity.h",
  "SecIdentitySearch.h",
  "SecImportExport.h",
  "SecItem.h",
  "SecKey.h",
  "SecKeychain.h",
  "SecKeychainItem.h",
  "SecKeychainSearch.h",
  "SecPolicy.h",
  "SecPolicySearch.h",
  "SecProtocolMetadata.h",
  "SecProtocolObject.h",
  "SecProtocolOptions.h",
  "SecProtocolTypes.h",
  "SecRandom.h",
  "SecReadTransform.h",
  "SecRequirement.h",
  "SecSharedCredential.h",
  "SecSignVerifyTransform.h",
  "SecStaticCode.h",
  "SecTask.h",
  "SecTransform.h",
  "SecTransformReadTransform.h",
  "SecTrust.h",
  "SecTrustedApplication.h",
  "SecTrustSettings.h",
  "SecureDownload.h",
  "SecureTransport.h",
  "Security.h",
  "x509defs.h",
];

/// Where objc2's crates go until [sdk] has read them.
const OBJC2: &str = "objc2/";

/// The folder of the SDK's frameworks, where ld64 looks for them.
const FRAMEWORKS: &str = "System/Library/Frameworks";

/// What CoreFoundation exports that objc2 leaves out: its variadic functions, which
/// core-foundation-sys binds, their `va_list` forms, and what its macros call for C, `CFSTR`'s
/// class of constant strings among them.
const CF_EXTRA: &[&str] = &[
  "CFCalendarAddComponents",
  "CFCalendarComposeAbsoluteTime",
  "CFCalendarDecomposeAbsoluteTime",
  "CFCalendarGetComponentDifference",
  "CFMakeCollectable",
  "CFStringAppendFormat",
  "CFStringAppendFormatAndArguments",
  "CFStringCreateWithFormat",
  "CFStringCreateWithFormatAndArguments",
  "__CFConstantStringClassReference",
  "__CFRangeMake",
  "__CFStringMakeConstantString",
];

/// IOKit's private functions, which objc2 leaves out and sysinfo binds for Apple silicon's
/// temperatures.
const IOKIT_PRIVATE: &[&str] = &[
  "IOHIDEventGetFloatValue",
  "IOHIDEventSystemClientCreate",
  "IOHIDEventSystemClientSetMatching",
  "IOHIDServiceClientCopyEvent",
];

const LIBCXX: Package = Package {
  mirrors: LLVM,
  path: "libcxx-20.1.8.src.tar.xz",
  sha256: "3e12992e3d1a0e648dc2e1c8c6b1dd31717ac8e684a25a302773667f47f75546",
  root: "",
  keep: &[],
  into: "",
};

/// libc++abi's headers, which libc++'s include.
const LIBCXXABI: Package = Package {
  path: "libcxxabi-20.1.8.src.tar.xz",
  sha256: "b4f6d3f9c802ea9e5db260e3e13177f63e8f8feef48618a8d53d4f7d4fcbb07f",
  ..LIBCXX
};

/// The SDK's folder of libc++'s headers, where clang looks for them in an SDK.
pub const LIBCXX_INCLUDE: &str = "usr/include/c++/v1/";

/// What libc++ calls on a failed check of its hardening, `__assertion_handler` once installed.
const ASSERTION_HANDLER: &str = "default_assertion_handler.in";

/// LLVM's lists of what macOS's libc++ exports for each arch, as a `.tbd` names it: its own, and
/// libc++abi's, which it re-exports.
const ABI_LISTS: [(&str, &str); 2] = [
  ("arm64", "arm64-apple-darwin.libcxxabi.v1.stable.exceptions.nonew.abilist"),
  ("x86_64", "x86_64-apple-darwin.libcxxabi.v1.stable.exceptions.nonew.abilist"),
];

/// libc++'s configuration as Apple's (`libcxx/cmake/caches/Apple.cmake`), whose availability marks
/// fail what the deployment target's libc++ lacks, as Xcode's; with no tzdb or libdispatch, which
/// the lists of its exports lack.
const CONFIG_SITE: &str = "/* cargo-solo's: libc++'s configuration, as Apple's. */
#ifndef _LIBCPP___CONFIG_SITE
#define _LIBCPP___CONFIG_SITE

#define _LIBCPP_ABI_VERSION 1
#define _LIBCPP_ABI_NAMESPACE __1
#define _LIBCPP_ABI_FORCE_ITANIUM 0
#define _LIBCPP_ABI_FORCE_MICROSOFT 0
#define _LIBCPP_HAS_THREADS 1
#define _LIBCPP_HAS_MONOTONIC_CLOCK 1
#define _LIBCPP_HAS_TERMINAL 1
#define _LIBCPP_HAS_MUSL_LIBC 0
#define _LIBCPP_HAS_THREAD_API_PTHREAD 1
#define _LIBCPP_HAS_THREAD_API_EXTERNAL 0
#define _LIBCPP_HAS_THREAD_API_WIN32 0
#define _LIBCPP_HAS_THREAD_API_C11 0
#define _LIBCPP_HAS_VENDOR_AVAILABILITY_ANNOTATIONS 1
#define _LIBCPP_HAS_FILESYSTEM 1
#define _LIBCPP_HAS_RANDOM_DEVICE 1
#define _LIBCPP_HAS_LOCALIZATION 1
#define _LIBCPP_HAS_UNICODE 1
#define _LIBCPP_HAS_WIDE_CHARACTERS 1
#define _LIBCPP_HAS_TIME_ZONE_DATABASE 0
#define _LIBCPP_INSTRUMENTED_WITH_ASAN 0
#define _LIBCPP_PSTL_BACKEND_SERIAL
#define _LIBCPP_HARDENING_MODE_DEFAULT _LIBCPP_HARDENING_MODE_NONE

#endif
";

/// The libraries an SDK has as links to libSystem, which has them all: the libc crate links `-lc`
/// and `-lm`.
const LIBSYSTEM_ALIASES: &[&str] =
  &["c", "dbm", "dl", "gcc_s.1", "info", "m", "mx", "mx.A", "poll", "proc", "pthread", "rpcsvc"];

/// A stub of macOS's libiconv, which the libc crate links for Apple's targets and Zig's source
/// doesn't have: the functions of `iconv.h`.
const LIBICONV: &str = "--- !tapi-tbd
tbd-version:     4
targets:         [ x86_64-macos, arm64-macos, arm64e-macos ]
install-name:    '/usr/lib/libiconv.2.dylib'
current-version: 7
compatibility-version: 7
exports:
  - targets:         [ x86_64-macos, arm64-macos, arm64e-macos ]
    symbols:         [ __libiconv_version, _iconv, _iconv_canonicalize, _iconv_close,
                       _iconv_open, _iconv_open_into, _iconvctl, _iconvlist,
                       _libiconv_set_relocation_prefix ]
...
";

/// A target's libstdc++, GCC 14's, linked statically: its packages, unpacked into a folder of their
/// own on the first C++ compile, and where they put its headers and libraries there.
#[derive(Debug)]
pub struct Cxx {
  pub packages: &'static [Package],
  /// The headers, `include/c++/<version>`.
  pub include: &'static str,
  /// GCC's name for the target: the folder of `include` with the target's own headers.
  pub triple: &'static str,
  /// The folder of `libstdc++.a`.
  pub lib: &'static str,
  /// Whether the folder has GCC 14's `libgcc.a`, for what clang++ links itself.
  pub libgcc: bool,
}

/// GCC 14's libgcc for aarch64: GCC 14's libstdc++ calls aarch64's outline atomics
/// (`__aarch64_ldadd4_acq_rel`), which GCC 8's libgcc lacks. rustc links its own,
/// compiler_builtins'.
const LIBGCC14_ARM64: Package = Package {
  root: "usr/lib/gcc/aarch64-linux-gnu/14/",
  keep: &["libgcc.a"],
  ..Package::deb(
    TRIXIE,
    "pool/main/g/gcc-14/libgcc-14-dev_14.2.0-19_arm64.deb",
    "64b8ebc7182a69aa9525df6eab5e7eb849b947e350d821549f8a8a3e9b19e5ba",
  )
};

/// libstdc++ for glibc 2.28 on x86_64: gcc-toolset-14's, AlmaLinux 8's, built for glibc 2.28. Not
/// its `libstdc++.so`, a linker script into the host's `/usr/lib64`, nor `libstdc++_nonshared.a`,
/// which goes with it.
pub const CXX_GLIBC_X64: Cxx = Cxx {
  packages: &[Package {
    mirrors: ALMA8,
    path: "x86_64/os/Packages/gcc-toolset-14-libstdc++-devel-14.2.1-11.el8_10.x86_64.rpm",
    sha256: "a5306cbaa5f3223de9320374aeac00dbddee817e11bab3c8217e33a131eca0a1",
    root: "opt/rh/gcc-toolset-14/root/usr/",
    keep: &[
      "include/c++/14/",
      "lib/gcc/x86_64-redhat-linux/14/libstdc++.a",
      "lib/gcc/x86_64-redhat-linux/14/libstdc++fs.a",
      "lib/gcc/x86_64-redhat-linux/14/libsupc++.a",
    ],
    into: "",
  }],
  include: "include/c++/14",
  triple: "x86_64-redhat-linux",
  lib: "lib/gcc/x86_64-redhat-linux/14",
  libgcc: false,
};

/// [CXX_GLIBC_X64] for aarch64.
pub const CXX_GLIBC_ARM64: Cxx = Cxx {
  packages: &[
    Package {
      mirrors: ALMA8,
      path: "aarch64/os/Packages/gcc-toolset-14-libstdc++-devel-14.2.1-11.el8_10.aarch64.rpm",
      sha256: "c70b6f1a0c67d4798e488772ce55dbceef0268a5ea551da83829965bd5f5235f",
      root: "opt/rh/gcc-toolset-14/root/usr/",
      keep: &[
        "include/c++/14/",
        "lib/gcc/aarch64-redhat-linux/14/libstdc++.a",
        "lib/gcc/aarch64-redhat-linux/14/libstdc++fs.a",
        "lib/gcc/aarch64-redhat-linux/14/libsupc++.a",
      ],
      into: "",
    },
    LIBGCC14_ARM64,
  ],
  include: "include/c++/14",
  triple: "aarch64-redhat-linux",
  lib: "lib/gcc/aarch64-redhat-linux/14",
  libgcc: true,
};

/// What a musl target keeps of Alpine's libstdc++-dev.
const ALPINE_CXX: &[&str] =
  &["include/c++/", "lib/libstdc++.a", "lib/libstdc++fs.a", "lib/libsupc++.a"];

/// libstdc++ for musl on x86_64: Alpine's, built for musl.
pub const CXX_MUSL_X64: Cxx = Cxx {
  packages: &[Package {
    mirrors: ALPINE,
    path: "x86_64/libstdc++-dev-14.2.0-r4.apk",
    sha256: "8a2c14b49e5b3d75ec32d6f62966df8b1abad200ab9479172805ddc0f221e1df",
    root: "usr/",
    keep: ALPINE_CXX,
    into: "",
  }],
  include: "include/c++/14.2.0",
  triple: "x86_64-alpine-linux-musl",
  lib: "lib",
  libgcc: false,
};

/// [CXX_MUSL_X64] for aarch64.
pub const CXX_MUSL_ARM64: Cxx = Cxx {
  packages: &[
    Package {
      mirrors: ALPINE,
      path: "aarch64/libstdc++-dev-14.2.0-r4.apk",
      sha256: "b347c2689cb1815510419dd68918c20649f48b0a7e665fea4cd8155dfa7adc9c",
      root: "usr/",
      keep: ALPINE_CXX,
      into: "",
    },
    LIBGCC14_ARM64,
  ],
  include: "include/c++/14.2.0",
  triple: "aarch64-alpine-linux-musl",
  lib: "lib",
  libgcc: true,
};

/// libstdc++ for x86_64 Windows: Debian's mingw-w64 g++, posix threads, only its headers and static
/// libraries: no `libstdc++.dll.a`, which would link the DLL.
pub const CXX_MINGW_X64: Cxx = Cxx {
  packages: &[Package {
    root: "usr/lib/gcc/x86_64-w64-mingw32/",
    keep: &[
      "14-posix/include/c++/",
      "14-posix/libstdc++.a",
      "14-posix/libstdc++fs.a",
      "14-posix/libsupc++.a",
    ],
    ..Package::deb(
      TRIXIE,
      "pool/main/g/gcc-mingw-w64/g++-mingw-w64-x86-64-posix_14.2.0-19+27+b1_amd64.deb",
      "cfa1773c2d0184819050138e587a81606d04a5f92e278b521b72b776d2c256c1",
    )
  }],
  include: "14-posix/include/c++",
  triple: "x86_64-w64-mingw32",
  lib: "14-posix",
  libgcc: false,
};

/// macOS's `sys/syscall.h`, which Zig's source doesn't have, without its numbers: jemalloc includes
/// it on every Unix, and syscall(2), deprecated on macOS, is what they're for.
const SYSCALL_H: &str = "/* cargo-solo's: macOS's header without the syscall numbers. */
#ifndef _SYS_SYSCALL_H_
#define _SYS_SYSCALL_H_
#endif
";

// MARK: Cache

/// The cache folder: `CARGO_SOLO_CACHE`, else the OS's own.
pub fn cache() -> io::Result<PathBuf> {
  let var = |name| env::var_os(name).filter(|x| !x.is_empty()).map(PathBuf::from);
  let dir = match (var("CARGO_SOLO_CACHE"), env::consts::OS) {
    (Some(dir), _) => Some(dir),
    (None, "macos") => var("HOME").map(|x| x.join("Library/Caches/cargo-solo")),
    (None, "windows") => var("LOCALAPPDATA").map(|x| x.join("cargo-solo")),
    (None, _) => {
      let base = var("XDG_CACHE_HOME").or_else(|| var("HOME").map(|x| x.join(".cache")));
      base.map(|x| x.join("cargo-solo"))
    }
  };

  // Absolute: the wrapper runs in each build script's folder.
  std::path::absolute(dir.ok_or_else(|| bad("no cache folder: set CARGO_SOLO_CACHE"))?)
}

/// The sysroot `name` of `packages` in `cache`, `<name>-<hash>`: a changed table or [FORMAT]
/// unpacks into another folder.
pub fn dir(cache: &Path, name: &str, packages: &[Package]) -> PathBuf {
  let mut text = format!("{FORMAT}");
  for x in packages {
    text += &format!("\n{} {} {} {}", x.sha256, x.root, x.keep.join(" "), x.into);
  }

  let hash = hex(&hmac_sha256::Hash::hash(text.as_bytes()));
  cache.join(format!("{name}-{}", &hash[..12]))
}

/// [dir], fetched and unpacked first when it's not there: into a temporary folder, renamed when
/// complete, so that a run cut short, or one beside, never sees half of it.
pub fn ensure(cache: &Path, name: &str, packages: &[Package]) -> io::Result<PathBuf> {
  let dir = dir(cache, name, packages);
  if dir.is_dir() {
    return Ok(dir);
  }

  let tmp = cache.join("tmp");
  fs::create_dir_all(&tmp)?;
  sweep(&tmp);

  // The time too: runs in containers that share the cache have the same ids.
  let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
  let part = tmp.join(format!("{name}-{}-{now}", std::process::id()));
  eprintln!("cargo-solo: fetching {name} into {}", dir.display());
  let done = fill(&part, packages).and_then(|()| fs::rename(&part, &dir));
  let _ = fs::remove_dir_all(&part);
  match done {
    // Another run put its own in place first.
    Err(_) if dir.is_dir() => Ok(dir),
    Err(e) => Err(e),
    Ok(()) => Ok(dir),
  }
}

/// Removes what runs cut short left in `tmp`: anything over a day old.
fn sweep(tmp: &Path) {
  let day = Duration::from_secs(24 * 3600);
  for entry in fs::read_dir(tmp).into_iter().flatten().flatten() {
    let age = entry.metadata().and_then(|x| x.modified()).ok().and_then(|x| x.elapsed().ok());
    if age.is_some_and(|x| x > day) {
      let _ = fs::remove_dir_all(entry.path());
    }
  }
}

fn fill(root: &Path, packages: &[Package]) -> io::Result<()> {
  fs::create_dir_all(root)?;
  let mut links = vec![];
  // Packages in a row that are parts of one file share its download.
  let mut last: Option<(&str, Vec<u8>)> = None;
  for pkg in packages {
    let file = pkg.path.rsplit('/').next().unwrap_or(pkg.path);
    let at = |e: io::Error| io::Error::new(e.kind(), format!("{file}: {e}"));
    let tar = match last.take() {
      Some((path, tar)) if path == pkg.path => tar,
      _ => {
        let data = fetch(pkg, &root.join(".download")).map_err(at)?;
        let tar = match file.rsplit('.').next() {
          Some("deb") => member(&data, "data.tar.xz").and_then(xz),
          Some("rpm") => payload(&data).and_then(xz),
          Some("apk" | "crate" | "gz") => gunzip(&data),
          _ => xz(&data),
        };

        tar.map_err(at)?
      }
    };

    // An rpm's is a cpio archive, not a tar.
    let got = match file.ends_with(".rpm") {
      true => cpio(&tar, |x| pkg.place(x), root),
      false => unpack(&tar, |x| pkg.place(x), root),
    };

    links.extend(got.map_err(at)?);
    last = Some((pkg.path, tar));
  }

  materialize(root, links)?;
  if root.join("usr/lib/libSystem.tbd").is_file() {
    sdk(root)?;
  }

  Ok(())
}

/// Adds to Zig's files of Apple's SDK libSystem's aliases, libiconv, `sys/syscall.h` and libobjc's
/// and the frameworks' `.tbd`s, and drops `arm64e.x1` from libSystem's stub: rust-lld before Rust
/// 1.100 can't read it. Configures libc++ as its build does, and makes its `.tbd` of the lists of
/// its exports.
fn sdk(root: &Path) -> io::Result<()> {
  let lib = root.join("usr/lib");
  let path = lib.join("libSystem.tbd");
  fs::write(&path, without(&fs::read_to_string(&path)?, "arm64e.x1-"))?;
  for name in LIBSYSTEM_ALIASES {
    copy(&path, &lib.join(format!("lib{name}.tbd")))?;
  }

  fs::write(lib.join("libiconv.tbd"), LIBICONV)?;
  let path = root.join("usr/include/sys/syscall.h");
  if !path.exists() {
    fs::write(path, SYSCALL_H)?;
  }

  // clang 18 counts C++20 as modules, and math.h then takes INFINITY and NAN from clang's float.h,
  // which has them from clang 19.
  let path = root.join("usr/include/math.h");
  fs::write(&path, fs::read_to_string(&path)?.replace("__has_feature(modules)", "0"))?;
  let include = root.join(LIBCXX_INCLUDE);
  fs::write(include.join("__config_site"), CONFIG_SITE)?;
  fs::rename(include.join(ASSERTION_HANDLER), include.join("__assertion_handler"))?;
  let mut lists = vec![];
  for (arch, name) in ABI_LISTS {
    lists.push((arch, fs::read_to_string(lib.join(name))?));
    fs::remove_file(lib.join(name))?;
  }

  let [cxx, abi] = libcxx_tbds(&lists);
  fs::write(lib.join("libc++.tbd"), cxx)?;
  fs::write(lib.join("libc++abi.tbd"), abi)?;

  // Swift's CoreFoundation is macOS's, but for Swift's runtime unless told: macOS's has its own
  // constant strings, and its umbrella takes none of Swift's headers.
  let cf = root.join(FRAMEWORKS).join("CoreFoundation.framework/Headers");
  let swift = "#define DEPLOYMENT_RUNTIME_SWIFT ";
  patch(&cf.join("CFAvailability.h"), &format!("{swift}1"), &format!("{swift}0"))?;
  patch(&cf.join("CoreFoundation.h"), "#include \"CFConstantKeys.h\"\n", "")?;
  patch(&cf.join("CoreFoundation.h"), "#include \"ForSwiftFoundationOnly.h\"\n", "")?;
  // CFStream's dispatch queues, which macOS has and Swift's build turns on: Security's headers take
  // dispatch through it.
  patch(&cf.join("CFStream.h"), "#if __HAS_DISPATCH__\n", "#if 1\n")?;

  // configd's and Security's headers as Apple's build installs them: without bridgeOS.
  let sc = SC_HEADERS.iter().map(|x| ("SystemConfiguration", x));
  for (fw, name) in sc.chain(SECURITY_HEADERS.iter().map(|x| ("Security", x))) {
    let path = root.join(FRAMEWORKS).join(format!("{fw}.framework/Headers")).join(name);
    fs::write(&path, no_bridgeos(&fs::read(&path)?))?;
  }

  // libDER's `DERItem`, the type of Security's OIDs, written out: libDER isn't open source.
  let der = "#include <stddef.h>\n#include <stdint.h>\n\
             typedef struct { uint8_t *data; size_t length; } DERItem;\n";
  let oids = root.join(FRAMEWORKS).join("Security.framework/Headers/oids.h");
  patch(&oids, "#include <libDER/DERItem.h>\n", der)?;

  frameworks(root)
}

/// `text` without bridgeOS in its availability marks, which only Apple's clang knows: `, bridgeos`
/// in a list, `bridgeos, ` first in one, or `API_UNAVAILABLE(bridgeos)`, each with its version.
/// Bytes, as some of Security's headers aren't UTF-8.
fn no_bridgeos(text: &[u8]) -> Vec<u8> {
  let (word, alone) = (b"bridgeos", b"API_UNAVAILABLE(");
  let mut text = text.to_vec();
  let mut from = 0;
  while let Some(x) = text[from..].windows(word.len()).position(|x| x == word) {
    let at = from + x;
    let mut end = at + word.len();
    if text.get(end) == Some(&b'(')
      && let Some(x) = text[end..].iter().position(|&x| x == b')')
    {
      end += x + 1;
    }

    let lead = text[..at].trim_ascii_end();
    let next = text[end..].trim_ascii_start();
    let span = match (lead.last(), next.first()) {
      (Some(b','), Some(b',' | b')')) => lead.len() - 1..end,
      (Some(b'('), Some(b',')) => at..text.len() - next[1..].trim_ascii_start().len(),
      (Some(b'('), Some(b')')) if lead.ends_with(alone) => {
        lead.len() - alone.len()..text.len() - next.len() + 1
      }
      _ => {
        from = end;
        continue;
      }
    };

    from = span.start;
    text.drain(span);
  }

  text
}

/// Replaces `from` in the file at `path` with `to`; fails when it hasn't `from`, as a newer pin
/// may not.
fn patch(path: &Path, from: &str, to: &str) -> io::Result<()> {
  let text = fs::read_to_string(path)?;
  if !text.contains(from) {
    return Err(bad(format!("{}: no {from:?}", path.display())));
  }

  fs::write(path, text.replace(from, to))
}

/// The `.tbd`s of macOS's libc++ and libc++abi from LLVM's lists of libc++'s exports per arch:
/// lines such as `{'is_defined': True, 'name': '__ZTIn', 'type': 'I'}`, type `I` libc++abi's, which
/// bind to it as Xcode's do below macOS 15.4: an older libc++ re-exports only some.
fn libcxx_tbds(lists: &[(&str, String)]) -> [String; 2] {
  let archs = lists.iter().map(|(arch, _)| format!("{arch}-macos")).collect::<Vec<_>>().join(", ");
  let head = |name| {
    format!(
      "--- !tapi-tbd\ntbd-version: 4\ntargets: [ {archs} ]\ninstall-name: '/usr/lib/{name}'\n"
    )
  };
  let reexport = format!(
    "reexported-libraries:\n  - targets: [ {archs} ]\n    \
     libraries: [ '/usr/lib/libc++abi.dylib' ]\n"
  );
  let mut out =
    [head("libc++.1.dylib") + &reexport, head("libc++abi.dylib")].map(|x| x + "exports:\n");
  let name = |x: &str| Some(format!("'{}'", x.split("'name': '").nth(1)?.split('\'').next()?));
  for (arch, list) in lists {
    let defined = list.lines().filter(|x| x.contains("'is_defined': True"));
    let (abi, cxx): (Vec<_>, Vec<_>) = defined.partition(|x| x.contains("'type': 'I'"));
    for (out, lines) in out.iter_mut().zip([cxx, abi]) {
      let names: Vec<_> = lines.into_iter().filter_map(name).collect();
      *out += &format!("  - targets: [ {arch}-macos ]\n    symbols: [ {} ]\n", names.join(", "));
    }
  }

  out.map(|x| x + "...\n")
}

/// Each framework's `.tbd` from the objc2 crate that binds it: what its `extern` blocks declare.
/// Only the names a link needs, none of Apple's files; the install name is the framework's on every
/// macOS, where dyld finds what an umbrella's subframeworks export, as FSEvents' in CoreServices.
fn frameworks(root: &Path) -> io::Result<()> {
  // A name libSystem exports stays its: a stub that lists it too, as objc2 binds `free`, would bind
  // it there, where dyld doesn't find it.
  let system = fs::read_to_string(root.join("usr/lib/libSystem.tbd"))?;
  let system: HashSet<_> = system.split([' ', ',', '\n', '[', ']', '\'']).collect();
  for entry in fs::read_dir(root.join(OBJC2))? {
    let dir = entry?.path();
    let config = fs::read_to_string(dir.join("translation-config.toml"))?;
    let name = config.lines().find_map(|x| x.strip_prefix("framework = \"")?.strip_suffix('"'));
    let name = name.ok_or_else(|| bad(format!("{}: no framework", dir.display())))?;

    // objc2's own crate binds libobjc, Objective-C's runtime, in its `ffi`. Foundation's only
    // version is C, as AppKit's; every other framework's, A.
    let framework = format!("{FRAMEWORKS}/{name}.framework");
    let version = if name == "Foundation" { "C" } else { "A" };
    let (path, install, src) = match name {
      "ObjectiveC" => ("usr/lib/libobjc.tbd".into(), "/usr/lib/libobjc.A.dylib".into(), "src/ffi"),
      _ => (
        format!("{framework}/{name}.tbd"),
        format!("/{framework}/Versions/{version}/{name}"),
        "src",
      ),
    };

    let mut symbols = externs_in(&dir.join(src))?;
    let extra = match name {
      "CoreFoundation" => CF_EXTRA,
      "IOKit" => IOKIT_PRIVATE,
      _ => &[],
    };

    symbols.extend(extra.iter().map(|x| x.to_string()));
    symbols.retain(|x| !system.contains(format!("_{x}").as_str()));
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap_or(root))?;
    fs::write(path, tbd(&install, symbols))?;
  }

  fs::remove_dir_all(root.join(OBJC2))
}

/// What the `.rs` files under `dir`, in its folders too, declare: their [externs].
fn externs_in(dir: &Path) -> io::Result<Vec<String>> {
  let mut out = vec![];
  for entry in fs::read_dir(dir)? {
    let path = entry?.path();
    match path.is_dir() {
      true => out.extend(externs_in(&path)?),
      false if path.extension().is_some_and(|x| x == "rs") => {
        out.extend(externs(&fs::read_to_string(path)?))
      }
      false => {}
    }
  }

  Ok(out)
}

/// The C names Rust's `source` declares in its `extern "C"` blocks, or objc2's macros of them: each
/// function's and static's, and each `link_name`, which may be x86_64's only.
fn externs(source: &str) -> Vec<String> {
  let code: Vec<_> = source.lines().map(|x| x.split("//").next().unwrap_or_default()).collect();
  let code = code.join("\n");
  let mut out = vec![];
  for abi in ["extern \"C\" {", "extern \"C-unwind\" {", "extern_c! {", "extern_c_unwind! {"] {
    for (i, _) in code.match_indices(abi) {
      let body = &code[i + abi.len()..];
      let mut depth = 1;
      let end = body.find(|x| {
        depth += match x {
          '{' => 1,
          '}' => -1,
          _ => 0,
        };
        depth == 0
      });
      let body = &body[..end.unwrap_or(body.len())];

      let strings = body.split('"');
      let links = body.match_indices("link_name = \"").filter_map(|(i, x)| {
        let rest = &body[i + x.len()..];
        Some(rest[..rest.find('"')?].to_string())
      });
      out.extend(links);
      // Words in strings, as a deprecation's note, aren't names.
      let code: String = strings.step_by(2).collect();
      for word in ["fn", "static"] {
        out.extend(names(&code, word).map(str::to_string));
      }
    }
  }

  out
}

/// The names after each keyword `word` in `code`: `fn x` and `static mut x`, not `fn(` or
/// `'static`.
fn names<'a>(code: &'a str, word: &'a str) -> impl Iterator<Item = &'a str> {
  let ident = |x: char| x.is_alphanumeric() || x == '_';
  code.match_indices(word).filter_map(move |(i, _)| {
    let rest = &code[i + word.len()..];
    let before = code[..i].chars().next_back();
    if before.is_some_and(|x| ident(x) || x == '\'') || !rest.starts_with(char::is_whitespace) {
      return None;
    }

    let rest = rest.trim_start();
    let rest = rest.strip_prefix("mut ").unwrap_or(rest).trim_start();
    let end = rest.find(|x| !ident(x)).unwrap_or(rest.len());
    (end > 0).then(|| &rest[..end])
  })
}

/// The `.tbd` of the library at `install` on macOS, which exports `symbols`, C's names, for both
/// arches.
fn tbd(install: &str, mut symbols: Vec<String>) -> String {
  symbols.sort();
  symbols.dedup();
  let symbols: Vec<_> = symbols.iter().map(|x| format!("'_{x}'")).collect();
  let targets = "[ x86_64-macos, arm64-macos, arm64e-macos ]";
  format!(
    "--- !tapi-tbd\ntbd-version: 4\ntargets: {targets}\n\
     install-name: '{install}'\nexports:\n\
     \x20 - targets: {targets}\n    symbols: [ {} ]\n...\n",
    symbols.join(", ")
  )
}

/// The `.tbd` text `tbd` without the targets that start with `arch`, each with its comma.
fn without(tbd: &str, arch: &str) -> String {
  let mut out = String::with_capacity(tbd.len());
  let mut rest = tbd;
  while let Some(i) = rest.find(arch) {
    let mut end = rest[i..].find([',', ' ', '\n', ']']).map_or(rest.len(), |x| i + x);
    match rest[..i].trim_end().strip_suffix(',') {
      Some(head) => out.push_str(head),
      // The first in its list: the comma after it.
      None => {
        out.push_str(&rest[..i]);
        end += usize::from(rest[end..].starts_with(','));
      }
    }

    rest = &rest[end..];
  }

  out.push_str(rest);
  out
}

// MARK: Fetch

/// The file of `pkg` from the first mirror that has the pinned SHA-256, downloaded by `curl` into
/// `path`. No `--retry-all-errors`: Debian 10's and RHEL 8's curl lack it.
fn fetch(pkg: &Package, path: &Path) -> io::Result<Vec<u8>> {
  let mut failed = vec![];
  for mirror in pkg.mirrors {
    let url = format!("{mirror}/{}", pkg.path);
    let mut curl = Command::new("curl");
    curl.args(["-fsSL", "--connect-timeout", "30", "--speed-limit", "1024", "--speed-time", "60"]);
    curl.args(["--retry", "2", "-C", "-", "-o"]).arg(path).arg(&url);

    // A download cut short goes on where it stopped, while each try gets more.
    let mut got = run(&mut curl);
    let mut size = 0;
    while let Err(_) = got
      && let Ok(x) = fs::metadata(path)
      && x.len() > size
    {
      size = x.len();
      got = run(&mut curl);
    }

    let got = got.and_then(|_| fs::read(path));
    let _ = fs::remove_file(path);
    match got {
      Ok(data) => {
        let sum = hex(&hmac_sha256::Hash::hash(&data));
        if sum == pkg.sha256 {
          return Ok(data);
        }

        failed.push(format!("{url}: SHA-256 {sum}, pinned {}", pkg.sha256));
      }
      // No curl: no mirror would do.
      Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(e),
      Err(e) => failed.push(format!("{url}: {e}")),
    }
  }

  Err(io::Error::other(failed.join("; ")))
}

pub fn hex(bytes: &[u8]) -> String {
  bytes.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
  use std::io::Write;

  use flate2::Compression;
  use flate2::write::GzEncoder;

  use super::*;
  use crate::toolchain::TARGETS;
  use crate::unpack::tests::{ar, tarball, temp};

  /// Every package of every target: its sysroot's and its libstdc++'s.
  fn every_package() -> impl Iterator<Item = &'static Package> {
    TARGETS.iter().flat_map(|x| x.packages.iter().chain(x.cxx.into_iter().flat_map(|x| x.packages)))
  }

  #[test]
  fn tables_are_pinned() {
    for pkg in every_package() {
      let hex = pkg.sha256.bytes().all(|x| x.is_ascii_digit() || (b'a'..=b'f').contains(&x));
      assert!(pkg.sha256.len() == 64 && hex, "{}", pkg.path);
      let kinds = [".deb", ".rpm", ".apk", ".crate", ".tar.xz", ".tar.gz"];
      assert!(kinds.iter().any(|x| pkg.path.ends_with(x)), "{}", pkg.path);
      assert!(pkg.mirrors.iter().all(|x| x.starts_with("https://") && !x.ends_with('/')));
      assert!(pkg.root.is_empty() || pkg.root.ends_with('/'));
    }

    // Each sysroot and libstdc++ its own folder: targets share one only by its name, as Apple's do.
    let cache = Path::new("/c");
    let sysroots =
      TARGETS.iter().map(|x| (x.sysroot().to_string(), dir(cache, x.sysroot(), x.packages)));
    let cxx = TARGETS
      .iter()
      .filter_map(|x| Some((x.libstdcxx(), dir(cache, &x.libstdcxx(), x.cxx?.packages))));
    let dirs: Vec<_> = sysroots.chain(cxx).collect();
    for (i, (name, x)) in dirs.iter().enumerate() {
      assert!(dirs[..i].iter().all(|(n, y)| (n == name) == (y == x)), "{}", x.display());
    }
  }

  #[test]
  fn dir_changes_with_the_packages() {
    let pkg = |sha256| [Package::deb(BUSTER, "a.deb", sha256)];
    let (a, b) = (pkg("aa"), pkg("bb"));
    let cache = Path::new("/c");
    assert_eq!(dir(cache, "x", &a), dir(cache, "x", &a));
    assert_ne!(dir(cache, "x", &a), dir(cache, "x", &b));
    assert!(dir(cache, "x", &a).starts_with("/c"));
    assert!(dir(cache, "x", &a).file_name().unwrap().to_str().unwrap().starts_with("x-"));
  }

  #[test]
  fn place_keeps_headers_and_libraries() {
    let pkg = Package::deb(BUSTER, "a.deb", "");
    assert_eq!(pkg.place("usr/include/stdio.h"), Some("usr/include/stdio.h".into()));
    let libc = "lib/x86_64-linux-gnu/libc.so.6";
    assert_eq!(pkg.place(libc), Some(libc.into()));
    assert_eq!(pkg.place("usr/share/doc/libc6/copyright"), None);
    assert_eq!(pkg.place("usr/bin/ldd"), None);
    assert_eq!(pkg.place("usr/lib/x86_64-linux-gnu/gconv/UTF-16.so"), None);
    assert_eq!(pkg.place("usr/lib"), None);

    let mingw = &MINGW_X64[2];
    let got = mingw.place(&format!("{}libgcc_eh.a", mingw.root));
    assert_eq!(got, Some("usr/x86_64-w64-mingw32/lib/libgcc_eh.a".into()));
    assert_eq!(mingw.place(&format!("{}libmsvcrt.a", mingw.root)), None);

    let (musl, gcc) = (&MUSL_X64[2], "usr/lib/gcc/x86_64-linux-gnu/8");
    let got = musl.place(&format!("{gcc}/crtbeginS.o"));
    assert_eq!(got, Some("usr/lib/x86_64-linux-musl/crtbeginS.o".into()));
    assert_eq!(musl.place(&format!("{gcc}/libgcc_s.so")), None);

    let libcxx = &DARWIN[2];
    assert_eq!(
      libcxx.place(&format!("{}vector", libcxx.root)),
      Some("usr/include/c++/v1/vector".into())
    );
    assert_eq!(libcxx.place(&format!("{}__cxx03/vector", libcxx.root)), None);
  }

  #[test]
  fn ensure_tries_the_next_mirror_and_checks_the_hash() {
    let root = temp("ensure");
    let tar = tarball(&[("./usr/include/a.h", "a")], &[]);
    let mut xz = vec![];
    lzma_rs::xz_compress(&mut &tar[..], &mut xz).unwrap();
    let file = ar(&[("debian-binary", b"2.0\n"), ("data.tar.xz", &xz)]);
    fs::create_dir_all(root.join("good/pool")).unwrap();
    fs::write(root.join("good/pool/a.deb"), &file).unwrap();

    let sha256: &str = hex(&hmac_sha256::Hash::hash(&file)).leak();
    let good: &str = format!("file://{}", root.join("good").display()).leak();
    let none: &str = format!("file://{}", root.join("none").display()).leak();
    let cache = root.join("cache");
    let pkgs = [Package::deb(vec![none, good].leak(), "pool/a.deb", sha256)];
    let dir = ensure(&cache, "x", &pkgs).unwrap();
    assert_eq!(fs::read_to_string(dir.join("usr/include/a.h")).unwrap(), "a");
    assert_eq!(fs::read_dir(cache.join("tmp")).unwrap().count(), 0);
    // Once there, it's not fetched again.
    fs::remove_file(root.join("good/pool/a.deb")).unwrap();
    assert_eq!(ensure(&cache, "x", &pkgs).unwrap(), dir);

    let wrong = [Package::deb(vec![good].leak(), "pool/a.deb", "00")];
    fs::write(root.join("good/pool/a.deb"), &file).unwrap();
    let err = ensure(&cache, "y", &wrong).unwrap_err().to_string();
    assert!(err.contains("a.deb: ") && err.contains(&format!("SHA-256 {sha256}, pinned 00")));
    assert!(!super::dir(&cache, "y", &wrong).exists());
    fs::remove_dir_all(root).unwrap();
  }

  #[test]
  fn ensure_lays_out_apple_s_sdk() {
    let root = temp("sdk");
    let at = |i: usize, x: &str| format!("{}{x}", DARWIN[i].root);
    let abi = |x| format!("{{'is_defined': True, 'name': '{x}', 'type': 'FUNC'}}\n");
    let (arm, x86) = (abi("__Zarm"), abi("__Zx86"));
    let config = |x| format!("framework = \"{x}\"\n");
    let (cf, sec) = (config("CoreFoundation"), config("Security"));
    // Each with the package whose file it's in: Zig's, LLVM's libc++ and libc++abi, objc2's,
    // Swift's CoreFoundation, configd, Security.
    let mut files = vec![
      (0, at(0, "stdio.h"), "stdio"),
      (0, at(0, "sys/types.h"), "types"),
      (0, at(0, "math.h"), "#if __has_feature(modules) && x"),
      (1, at(1, "libSystem.tbd"), "[ arm64e-macos, arm64e.x1-macos ]\n[ _free ]"),
      // Not taken: one that the sysroot leaves out, and another libc's.
      (1, at(1, "SDKSettings.json"), "{}"),
      (1, DARWIN[1].root.replace("/darwin/", "/glibc/") + "stdio.h", "glibc"),
      (2, at(2, "vector"), "vector"),
      (3, at(3, ASSERTION_HANDLER), "handler"),
      (4, at(4, ABI_LISTS[0].1), &arm),
      (4, at(4, ABI_LISTS[1].1), &x86),
      (4, at(4, "x86_64-unknown-linux-gnu.libcxxabi.v1.stable.exceptions.nonew.abilist"), "linux"),
      (5, at(5, "cxxabi.h"), "cxxabi"),
      (5, at(5, "CMakeLists.txt"), "cmake"),
      (6, "objc2-0.6.3/translation-config.toml".into(), "framework = \"ObjectiveC\"\n"),
      (6, "objc2-0.6.3/src/ffi/rc.rs".into(), "extern_c! { pub fn objc_retain(); pub fn free(); }"),
      (6, "objc2-0.6.3/src/lib.rs".into(), "extern \"C\" { pub fn not_ffi(); }"),
      (7, "cf-0.3.2/translation-config.toml".into(), &cf),
      (7, "cf-0.3.2/src/generated/CFArray.rs".into(), "extern \"C\" { pub fn CFArrayCreate(); }"),
      (12, "sec-0.3.2/translation-config.toml".into(), &sec),
      (12, "sec-0.3.2/src/lib.rs".into(), "extern \"C\" { pub static kSecClass: u8; }"),
    ];
    for i in [8, 9, 10, 11, 13] {
      files.push((i, format!("x{i}/translation-config.toml"), "framework = \"X\"\n"));
      files.push((i, format!("x{i}/src/lib.rs"), ""));
    }

    let umbrella = "#include \"CFBase.h\"\n#include \"CFConstantKeys.h\"\n\
                    #include \"ForSwiftFoundationOnly.h\"\n";
    files.extend([
      (14, at(14, "CFAvailability.h"), "#define DEPLOYMENT_RUNTIME_SWIFT 1\n"),
      (14, at(14, "CoreFoundation.h"), umbrella),
      (14, at(14, "CFStream.h"), "#if __HAS_DISPATCH__\n#include <dispatch/dispatch.h>\n"),
      // Not taken: Swift's own.
      (14, at(14, "ForSwiftFoundationOnly.h"), "swift"),
      (14, at(14, "module.modulemap"), "module"),
      (15, at(15, "SCPrivate.h"), "private"),
    ]);
    let sc = "f(void) API_UNAVAILABLE(ios, bridgeos) SPI_AVAILABLE(ios(2.0), bridgeos(1.0));";
    files.extend(SC_HEADERS.iter().map(|x| (15, at(15, x), sc)));

    let mine = ["SecBase.h", "oids.h", "SecKeychain.h"];
    let others = SECURITY_HEADERS.iter().filter(|x| !mine.contains(x));
    files.extend(others.map(|x| (16, at(16, x), "")));
    files.extend([
      (16, at(16, "SecBase.h"), "API_UNAVAILABLE(ios, bridgeos, macCatalyst)"),
      (16, at(16, "oids.h"), "#include <libDER/DERItem.h>\nextern const DERItem oidRsa;\n"),
      // Not taken: a private one.
      (16, at(16, "SecItemPriv.h"), "private"),
      (17, at(17, "SecKeychain.h"), "API_AVAILABLE(macos(1), bridgeos(NA))"),
    ]);

    let mirror: &str = format!("file://{}", root.display()).leak();
    let mut pkgs = vec![];
    for pkg in DARWIN {
      let mine = files.iter().filter(|(i, ..)| DARWIN[*i].path == pkg.path);
      let tar = tarball(&mine.map(|(_, x, data)| (x.as_str(), *data)).collect::<Vec<_>>(), &[]);
      let data = match pkg.path.ends_with(".crate") || pkg.path.ends_with(".gz") {
        true => {
          let mut gz = GzEncoder::new(vec![], Compression::default());
          gz.write_all(&tar).unwrap();
          gz.finish().unwrap()
        }
        false => {
          let mut xz = vec![];
          lzma_rs::xz_compress(&mut &tar[..], &mut xz).unwrap();
          xz
        }
      };

      let path = root.join(pkg.path);
      fs::create_dir_all(path.parent().unwrap()).unwrap();
      fs::write(path, &data).unwrap();
      let sha256 = hex(&hmac_sha256::Hash::hash(&data)).leak();
      pkgs.push(Package { mirrors: vec![mirror].leak(), sha256, ..*pkg });
    }

    let dir = ensure(&root.join("cache"), "macos", &pkgs).unwrap();
    let read = |x: &str| fs::read_to_string(dir.join(x)).unwrap();
    assert_eq!(read("usr/include/stdio.h"), "stdio");
    assert_eq!(read("usr/include/sys/types.h"), "types");
    assert_eq!(read("usr/include/math.h"), "#if 0 && x");
    assert_eq!(read("usr/lib/libSystem.tbd"), "[ arm64e-macos ]\n[ _free ]");
    assert_eq!(read("usr/lib/libc.tbd"), "[ arm64e-macos ]\n[ _free ]");
    assert_eq!(read("usr/lib/libm.tbd"), "[ arm64e-macos ]\n[ _free ]");
    assert!(read("usr/lib/libiconv.tbd").contains("'/usr/lib/libiconv.2.dylib'"));
    assert!(read("usr/include/sys/syscall.h").contains("_SYS_SYSCALL_H_"));

    let tbd = read("usr/lib/libc++.tbd");
    assert!(tbd.contains("[ arm64-macos ]\n    symbols: [ '__Zarm' ]\n"));
    assert!(tbd.contains("[ x86_64-macos ]\n    symbols: [ '__Zx86' ]\n"));
    assert!(read("usr/lib/libc++abi.tbd").contains("'/usr/lib/libc++abi.dylib'"));

    let v1 = "usr/include/c++/v1/";
    assert_eq!(read(&format!("{v1}vector")), "vector");
    assert_eq!(read(&format!("{v1}__assertion_handler")), "handler");
    assert_eq!(read(&format!("{v1}cxxabi.h")), "cxxabi");
    assert!(read(&format!("{v1}__config_site")).contains("_AVAILABILITY_ANNOTATIONS 1\n"));

    let cf = read("System/Library/Frameworks/CoreFoundation.framework/CoreFoundation.tbd");
    let install = "'/System/Library/Frameworks/CoreFoundation.framework/Versions/A/CoreFoundation'";
    assert!(cf.contains(&format!("install-name: {install}\n")));
    assert!(cf.contains("symbols: [ '_CFArrayCreate', '_CFCalendarAddComponents', "));
    assert!(cf.contains("'___CFConstantStringClassReference'"));
    let sec = read("System/Library/Frameworks/Security.framework/Security.tbd");
    assert!(sec.contains("symbols: [ '_kSecClass' ]\n"));
    let objc = read("usr/lib/libobjc.tbd");
    assert!(objc.contains("install-name: '/usr/lib/libobjc.A.dylib'\n"));
    assert!(objc.contains("symbols: [ '_objc_retain' ]\n"));

    let names = |x: &str| {
      let mut out: Vec<_> =
        fs::read_dir(dir.join(x)).unwrap().map(|x| x.unwrap().file_name()).collect();
      out.sort();
      out
    };
    assert_eq!(names(""), ["System", "usr"]);
    assert_eq!(names("usr"), ["include", "lib"]);
    assert!(names("usr/lib").iter().all(|x| !x.to_string_lossy().ends_with(".abilist")));
    assert_eq!(names(v1), ["__assertion_handler", "__config_site", "cxxabi.h", "vector"]);
    let frameworks =
      ["CoreFoundation.framework", "Security.framework", "SystemConfiguration.framework"];
    assert_eq!(names(FRAMEWORKS), [&frameworks[..], &["X.framework"]].concat());

    let headers = format!("{FRAMEWORKS}/CoreFoundation.framework/Headers/");
    assert_eq!(names(&headers), ["CFAvailability.h", "CFStream.h", "CoreFoundation.h"]);
    assert_eq!(read(&format!("{headers}CFStream.h")), "#if 1\n#include <dispatch/dispatch.h>\n");
    assert_eq!(read(&format!("{headers}CFAvailability.h")), "#define DEPLOYMENT_RUNTIME_SWIFT 0\n");
    assert_eq!(read(&format!("{headers}CoreFoundation.h")), "#include \"CFBase.h\"\n");

    let headers = format!("{FRAMEWORKS}/SystemConfiguration.framework/Headers/");
    assert_eq!(names(&headers), SC_HEADERS);
    let sc = "f(void) API_UNAVAILABLE(ios) SPI_AVAILABLE(ios(2.0));";
    assert_eq!(read(&format!("{headers}SCDynamicStore.h")), sc);

    let headers = format!("{FRAMEWORKS}/Security.framework/Headers/");
    let mut all = SECURITY_HEADERS.to_vec();
    all.sort();
    assert_eq!(names(&headers), all);
    assert_eq!(read(&format!("{headers}SecBase.h")), "API_UNAVAILABLE(ios, macCatalyst)");
    assert_eq!(read(&format!("{headers}SecKeychain.h")), "API_AVAILABLE(macos(1))");
    assert!(read(&format!("{headers}oids.h")).contains("size_t length; } DERItem;\nextern"));
    fs::remove_dir_all(root).unwrap();
  }

  #[test]
  fn no_bridgeos_drops_it_from_availability() {
    let cases = [
      ("API_UNAVAILABLE(ios, bridgeos, macCatalyst)", "API_UNAVAILABLE(ios, macCatalyst)"),
      (
        "API_AVAILABLE(macos(10.7), ios(NA), bridgeos(NA));",
        "API_AVAILABLE(macos(10.7), ios(NA));",
      ),
      ("SPI_AVAILABLE(bridgeos(1.0), ios(2.0))", "SPI_AVAILABLE(ios(2.0))"),
      ("f(void)\n    API_UNAVAILABLE(bridgeos);", "f(void)\n    ;"),
      ("/* bridgeos */ API_UNAVAILABLE(ios , bridgeos)", "/* bridgeos */ API_UNAVAILABLE(ios )"),
      ("\u{d5} bridgeos", "\u{d5} bridgeos"),
    ];
    for (text, want) in cases {
      assert_eq!(String::from_utf8(no_bridgeos(text.as_bytes())).unwrap(), want, "{text}");
    }

    assert_eq!(no_bridgeos(b"\xd5 x, bridgeos)"), b"\xd5 x)");
  }

  #[test]
  fn libcxx_tbds_list_each_arch_s_exports() {
    let list = |x| {
      format!(
        "{{'is_defined': True, 'name': '{x}', 'type': 'FUNC'}}
{{'is_defined': False, 'name': '_free', 'type': 'U'}}
{{'is_defined': False, 'name': '___cxa_throw', 'type': 'U'}}
{{'is_defined': True, 'name': '___cxa_throw', 'type': 'I'}}
"
      )
    };
    let cxx = "--- !tapi-tbd
tbd-version: 4
targets: [ arm64-macos, x86_64-macos ]
install-name: '/usr/lib/libc++.1.dylib'
reexported-libraries:
  - targets: [ arm64-macos, x86_64-macos ]
    libraries: [ '/usr/lib/libc++abi.dylib' ]
exports:
  - targets: [ arm64-macos ]
    symbols: [ '__Za' ]
  - targets: [ x86_64-macos ]
    symbols: [ '__Zb' ]
...
";
    let abi = "--- !tapi-tbd
tbd-version: 4
targets: [ arm64-macos, x86_64-macos ]
install-name: '/usr/lib/libc++abi.dylib'
exports:
  - targets: [ arm64-macos ]
    symbols: [ '___cxa_throw' ]
  - targets: [ x86_64-macos ]
    symbols: [ '___cxa_throw' ]
...
";
    assert_eq!(libcxx_tbds(&[("arm64", list("__Za")), ("x86_64", list("__Zb"))]), [cxx, abi]);
  }

  #[test]
  fn externs_takes_what_extern_blocks_declare() {
    let source = r#"
/// fn not_doc()
extern "C" {
    #[deprecated = "use fn not_note instead"]
    pub fn CFArrayCreate(cb: Option<unsafe extern "C-unwind" fn(not_arg: i32)>);
    pub static kCFAllocatorDefault: Option<&'static CFAllocator>;
    static mut kMut: u8; // fn not_comment()
}
pub unsafe extern "C-unwind" fn not_wrapper() -> u8 {
    extern "C-unwind" {
        #[cfg_attr(target_arch = "x86_64", link_name = "SecKey$LEGACYMAC")]
        fn SecKey() -> u8;
    }
    unsafe { SecKey() }
}
fn not_rust() {}
"#;
    let mut got = externs(source);
    got.sort();
    assert_eq!(got, ["CFArrayCreate", "SecKey", "SecKey$LEGACYMAC", "kCFAllocatorDefault", "kMut"]);
  }

  #[test]
  fn without_drops_a_target_from_each_list() {
    let tbd =
      "targets:  [ x86_64-macos, arm64e-macos, \n    arm64e.x1-macos, arm64e.x1-maccatalyst ]
  - targets: [ arm64e.x1-macos, arm64-macos ]
    symbols: [ _a, _b ]";
    let want = "targets:  [ x86_64-macos, arm64e-macos ]
  - targets: [  arm64-macos ]
    symbols: [ _a, _b ]";
    assert_eq!(without(tbd, "arm64e.x1-"), want);
    assert_eq!(without("[ a ]", "arm64e.x1-"), "[ a ]");
  }

  #[test]
  #[ignore = "downloads every package from every mirror, ~240 MB"]
  fn every_package_is_on_every_mirror() {
    let root = temp("mirrors");
    let mut done = vec![];
    for pkg in every_package() {
      for mirror in pkg.mirrors {
        // Shared by targets, or one file's parts.
        if done.contains(&(mirror, pkg.path)) {
          continue;
        }

        let one = Package { mirrors: vec![*mirror].leak(), ..*pkg };
        fetch(&one, &root.join("file")).unwrap();
        done.push((mirror, pkg.path));
      }
    }

    fs::remove_dir_all(root).unwrap();
  }
}
