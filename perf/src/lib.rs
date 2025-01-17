#![cfg_attr(feature = "frozen-abi", feature(min_specialization))]
pub mod cuda_runtime;
pub mod data_budget;
pub mod deduper;
pub mod discard;
pub mod packet;
pub mod perf_libs;
pub mod recycler;
pub mod recycler_cache;
pub mod sigverify;
#[cfg(feature = "dev-context-only-utils")]
pub mod test_tx;
pub mod thread;
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate log;
#[cfg(test)]
#[macro_use]
extern crate assert_matches;
#[macro_use]
extern crate solana_metrics;
#[cfg_attr(feature = "frozen-abi", macro_use)]
#[cfg(feature = "frozen-abi")]
extern crate solana_frozen_abi_macro;
fn is_rosetta_emulated() -> bool {
    #[cfg(target_os = "macos")]
    {
        use std::str::FromStr;
        std::process::Command::new("sysctl")
            .args(["-in", "sysctl.proc_translated"])
            .output()
            .map_err(|_| ())
            .and_then(|output| String::from_utf8(output.stdout).map_err(|_| ()))
            .and_then(|stdout| u8::from_str(stdout.trim()).map_err(|_| ()))
            .map(|enabled| enabled == 1)
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

// **Rosetta 2** 和 **AVX/AVX2** 都是计算机硬件和软件架构相关的术语，分别与 **苹果的 M1/M2 芯片** 和 **Intel x86 处理器** 的指令集有关。下面我会分别介绍它们的概念及其关系。
// ### 1. **Rosetta 2 模拟环境**
// **Rosetta 2** 是苹果为其新一代基于 ARM 架构的 **M1/M2** 芯片（Apple Silicon）设计的一个翻译层（即模拟环境）。它的目的是允许原本为 **Intel x86 处理器** 编写的软件在 Apple Silicon 芯片上继续运行。简单来说，**Rosetta 2** 是苹果提供的一个 **二进制翻译器**，它将 x86 架构的指令转化为 ARM 架构的指令，从而使得不支持原生 ARM 架构的软件可以在新的 Apple Silicon 设备上运行。
// - **背景**：苹果公司从 2020 年起推出了基于自家 ARM 架构的 M1 芯片，并逐步将其应用于 Mac 系列产品。由于过渡期内仍有大量基于 x86 架构的软件存在，苹果推出了 Rosetta 2，以便这些软件能够在新的 ARM 架构上继续使用，而无需开发新的版本。
// - **Rosetta 2 的作用**：
//   - 它的目的是 **无缝支持** 不同架构之间的兼容性问题，特别是 x86 到 ARM 的过渡。
//   - **透明模拟**：用户无需直接干预，Rosetta 2 会在后台自动将 x86 指令翻译成适合 ARM 架构的指令。
//   - 这对于开发者来说，是一个临时解决方案，帮助他们过渡到新的硬件架构，在开发过程中无需立刻重写原本的 x86 代码。
// - **限制**：Rosetta 2 并不是完美的，它可能会带来一定的性能损失，尤其是对于那些大量依赖 CPU 计算的应用程序。
// ### 2. **AVX/AVX2**
// **AVX**（Advanced Vector Extensions）和 **AVX2** 是由 **Intel** 和 **AMD** 处理器支持的高级 **SIMD（单指令多数据）** 指令集，用于加速计算密集型任务，特别是浮点数计算和大规模的数据处理。它们通常用于处理大规模科学计算、数据分析、视频编码、图像处理等高性能计算任务。
// - **AVX**：
//   - **AVX** 是 Intel 在其处理器中引入的 SIMD 指令集，用于加速向量运算。AVX 扩展了早期的 **SSE**（Streaming SIMD Extensions）指令集，支持 **256 位向量**操作，允许一次执行更多的计算，提升了 CPU 的并行计算能力。
//   - AVX 支持浮点数和整数运算的并行处理，特别适合需要大量并行计算的应用场景（如科学计算、图形处理、音频/视频编码等）。
// - **AVX2**：
//   - **AVX2** 是 AVX 的继任版本，进一步扩展了 AVX 指令集。与 AVX 相比，AVX2 支持 **更广泛的指令**，包括对整数运算和更高效的内存访问模式的优化。
//   - AVX2 允许更高效的 **数据加载与存储**，并提供对 **256 位向量运算** 的支持，特别是在整数运算中大大提升了性能。
//   - 它对 **CPU 执行浮点和整数计算** 都有提升，尤其是在大量数据处理时，能显著减少运算时间。
// ### 3. **Rosetta 2 和 AVX/AVX2 的关系**
// **Rosetta 2** 和 **AVX/AVX2** 之间的关系主要体现在它们对不同架构的兼容性上：
// - **Rosetta 2** 是一个 **模拟器**，允许 **x86 架构的应用程序** 在 **ARM 架构**（如 M1/M2 芯片）上运行。
// - 由于 **AVX/AVX2** 是专为 **x86 架构**（特别是 Intel 和 AMD 处理器）设计的指令集，它们不适用于 **ARM 架构** 的处理器。因此，当一个支持 AVX/AVX2 指令集的软件在 Rosetta 2 环境下运行时，Rosetta 2 会将这些 x86 指令翻译成 ARM 架构的指令，但这种翻译会带来 **性能损失**，因为 AVX 和 AVX2 特性并不适用于 ARM 架构。
// ### 4. **AVX/AVX2 对性能的影响**
// - **AVX** 和 **AVX2** 指令集的引入，极大地提升了处理器的浮点运算性能。比如，在进行科学计算、图像处理、视频编解码等应用中，使用这些指令集可以大幅度加速计算过程。
// - 如果你的程序需要使用 AVX 或 AVX2 来优化性能，那么 **Rosetta 2** 可能会使性能大打折扣。因为 Rosetta 2 无法直接使用 AVX/AVX2 指令集，它只能将 x86 指令转译为 ARM 指令，这会导致程序运行速度降低，尤其是在高负载的情况下。
// ### 5. **总结**
// - **Rosetta 2** 是苹果的二进制翻译层，它使得旧版的 x86 架构程序能够在新的 ARM 架构的 Apple Silicon 设备上运行。
// - **AVX/AVX2** 是 Intel 和 AMD 处理器的指令集，用于加速高性能计算任务，特别是浮点运算和大规模的数据处理。
// - **Rosetta 2** 与 **AVX/AVX2** 的关系体现在，如果你在基于 ARM 的 Apple Silicon 上运行依赖 AVX 或 AVX2 的应用，性能可能会受到 Rosetta 2 的影响，因为 Rosetta 2 无法直接支持这些指令集。

/// 打印 cuda 和 avx 情况
pub fn report_target_features() {
    warn!(
        "CUDA is {}abled",
        if crate::perf_libs::api().is_some() {
            "en"
        } else {
            "dis"
        }
    );
    // Validator binaries built on a machine with AVX support will generate invalid opcodes
    // when run on machines without AVX causing a non-obvious process abort.  Instead detect
    // the mismatch and error cleanly.
    if !is_rosetta_emulated() {
        #[cfg(all(
            any(target_arch = "x86", target_arch = "x86_64"),
            build_target_feature_avx
        ))]
        {
            if is_x86_feature_detected!("avx") {
                info!("AVX detected");
            } else {
                error!(
                "Incompatible CPU detected: missing AVX support. Please build from source on the target"
            );
                std::process::abort();
            }
        }
        #[cfg(all(
            any(target_arch = "x86", target_arch = "x86_64"),
            build_target_feature_avx2
        ))]
        {
            if is_x86_feature_detected!("avx2") {
                info!("AVX2 detected");
            } else {
                error!(
                    "Incompatible CPU detected: missing AVX2 support. Please build from source on the target"
                );
                std::process::abort();
            }
        }
    }
}
