"""Choose where speech runs. Tehuti installs nothing; it uses the CUDA libraries it finds."""
import ctypes
import os
import sys
from dataclasses import dataclass
from pathlib import Path

# CTranslate2 4.x loads these by soname: https://opennmt.net/CTranslate2/installation.html
CUDA_LIBRARIES = (
    ("libcublas.so.12", "cublas", "nvidia-cublas-cu12"),
    ("libcudnn.so.9", "cudnn", "nvidia-cudnn-cu12"),
    ("libnvrtc.so.12", "cuda_nvrtc", "nvidia-cuda-nvrtc-cu12"),
)
CUDA_TYPES_BEST_FIRST = ("int8_float16", "int8", "int8_float32", "float32")
CPU_TYPE = "int8"


@dataclass
class Choice:
    device: str
    compute_type: str
    model: object
    cuda_error: str = ""


def choose_device(supported_cuda_types, load):
    """Load the model with the best type the card supports, else on the CPU."""
    error = ""
    for compute_type in CUDA_TYPES_BEST_FIRST:
        if compute_type in supported_cuda_types:
            try:
                return Choice("cuda", compute_type, load("cuda", compute_type))
            except Exception as failure:  # noqa: BLE001 - any CUDA failure falls back
                error = str(failure)
                break
    return Choice("cpu", CPU_TYPE, load("cpu", CPU_TYPE), error)


def cuda_library_dirs(site_packages):
    return [
        str(lib)
        for site in site_packages
        for _, name, _ in CUDA_LIBRARIES
        if (lib := Path(site) / "nvidia" / name / "lib").is_dir()
    ]


def library_path(dirs, current):
    present = [entry for entry in current.split(":") if entry]
    return ":".join([entry for entry in dirs if entry not in present] + present)


def missing_libraries(loadable):
    return [(soname, package) for soname, _, package in CUDA_LIBRARIES if not loadable(soname)]


def missing_library_warning(missing):
    if not missing:
        return None
    names = ", ".join(f"{soname} (pip package {package})" for soname, package in missing)
    return f"NVIDIA GPU found but CUDA libraries are missing: {names}; speech runs on the CPU"


def loadable(soname):
    try:
        ctypes.CDLL(soname)
    except OSError:
        return False
    return True


def reexec_with_cuda_libraries():
    """The dynamic loader reads LD_LIBRARY_PATH only at process start, so re-exec once."""
    import site
    dirs = cuda_library_dirs(site.getsitepackages())
    current = os.environ.get("LD_LIBRARY_PATH", "")
    path = library_path(dirs, current)
    if path != current:
        os.execve(sys.executable, [sys.executable, *sys.argv], {**os.environ, "LD_LIBRARY_PATH": path})


def select(model_name, log):
    import ctranslate2
    from faster_whisper import WhisperModel

    supported = set()
    if os.environ.get("TEHUTI_SPEECH_DEVICE", "") != "cpu":
        try:
            gpus = ctranslate2.get_cuda_device_count()
        except Exception:  # noqa: BLE001 - no driver means no GPU
            gpus = 0
        if gpus:
            warning = missing_library_warning(missing_libraries(loadable))
            if warning:
                log(warning, level="WARN")
            else:
                supported = set(ctranslate2.get_supported_compute_types("cuda"))

    def load(kind, compute_type):
        import numpy
        model = WhisperModel(model_name, device=kind, compute_type=compute_type)
        list(model.transcribe(numpy.zeros(16000, dtype=numpy.float32))[0])
        return model

    choice = choose_device(supported, load)
    if choice.cuda_error:
        log(f"CUDA failed, speech runs on the CPU: {choice.cuda_error}", level="WARN")
    log(f"speech runs on {choice.device} with {choice.compute_type}")
    return choice
