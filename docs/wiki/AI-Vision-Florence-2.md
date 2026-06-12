# AI Vision Florence-2

Media Buddy uses Florence-2 ONNX for local image captioning and object tags.

## What It Can Do

- Generate image captions.
- Generate object-derived tags.
- Fill missing captions.
- Replace short captions when explicitly configured.
- Add tags while preserving existing provider tags.

Vision currently supports image files, not video files.

## First Load

Florence-2 is not bundled inside the exe. Press **Load** in Settings to download
and cache model/runtime files in:

```text
data/models/
```

The first load can take time. Later loads reuse cached files.

## Runtime Downloads

The app can download runtime files used by ONNX Runtime:

- ONNX Runtime CPU from Microsoft GitHub releases.
- DirectML provider package from NuGet.
- CUDA provider package from NuGet.
- cuDNN runtime files from PyPI.

CUDA acceleration still requires compatible NVIDIA drivers and CUDA 12 runtime
DLLs available to Windows.

## Execution Modes

| Mode | Behavior |
| --- | --- |
| Auto | Prefer compatible GPU workers, use CPU only if selected/needed by fallback. |
| CUDA | Use NVIDIA CUDA workers when available. |
| DirectML | Use DirectML workers when available. |
| CPU | Use CPU workers only. |

## Worker Planning

Media Buddy detects available GPUs and estimates worker capacity from VRAM and
the configured reserved VRAM value. Florence-2 workers are separate loaded model
instances. Multiple workers allow more parallel image jobs.

Important settings:

- **GPU instances per GPU**: upper bound per compatible GPU.
- **Max total instances**: global upper bound.
- **Reserved VRAM (GB)**: memory left unused per GPU.
- **CPU instances**: number of CPU workers when CPU mode or fallback is used.
- **CPU threads per instance**: `0` means automatic runtime thread behavior.

## CPU Fallback

CPU fallback is for cases where GPU workers cannot be planned or loaded. If GPU
workers are successfully loaded in GPU mode, fallback CPU workers should not be
added just because the checkbox is enabled.

## Caption Safety

Florence output can be useful but should not blindly overwrite good provider
metadata. Use the Images tab caption controls to choose whether AI should:

- Fill only missing captions.
- Replace captions that are shorter than a configured threshold.
- Overwrite captions.
- Add object tags.

## Troubleshooting Vision

- If DirectML reports unsupported feature level, try CUDA or CPU.
- If CUDA reports missing DLLs, install CUDA 12 runtime components or switch to
  CPU.
- If CUDA reports invalid resource handles, unload, restart the app, and try a
  lower worker count.
- If a model file hash mismatch is reported, delete `data/models/` and load
  again from a trusted network.
- If you switch runtime families after loading, restart the app. ONNX Runtime is
  process-wide and may not fully switch providers in one process.
