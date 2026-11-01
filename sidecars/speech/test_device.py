import tempfile
import unittest
from pathlib import Path

import device

PASCAL_TYPES = {"float32", "int8", "int8_float32"}
TURING_TYPES = {"float32", "float16", "int8", "int8_float16", "int8_float32"}


class Loader:
    def __init__(self, working):
        self.working = working
        self.attempts = []

    def __call__(self, kind, compute_type):
        self.attempts.append((kind, compute_type))
        if (kind, compute_type) not in self.working:
            raise RuntimeError(f"{kind} {compute_type} rejected")
        return f"model on {kind} {compute_type}"


class ChooseDeviceTest(unittest.TestCase):
    def test_pascal_card_runs_int8_on_cuda_and_never_asks_for_float16(self):
        load = Loader({("cuda", "int8")})
        choice = device.choose_device(PASCAL_TYPES, load)
        self.assertEqual((choice.device, choice.compute_type), ("cuda", "int8"))
        self.assertEqual(choice.model, "model on cuda int8")
        self.assertNotIn("float16", {compute for _, compute in load.attempts})

    def test_newer_card_runs_its_best_supported_type(self):
        load = Loader({("cuda", "int8_float16"), ("cuda", "int8")})
        choice = device.choose_device(TURING_TYPES, load)
        self.assertEqual((choice.device, choice.compute_type), ("cuda", "int8_float16"))

    def test_no_gpu_runs_on_the_cpu(self):
        load = Loader({("cpu", "int8")})
        choice = device.choose_device(set(), load)
        self.assertEqual((choice.device, choice.compute_type), ("cpu", "int8"))
        self.assertEqual(load.attempts, [("cpu", "int8")])

    def test_cuda_that_fails_to_load_falls_back_to_the_cpu(self):
        load = Loader({("cpu", "int8")})
        choice = device.choose_device(TURING_TYPES, load)
        self.assertEqual((choice.device, choice.compute_type), ("cpu", "int8"))
        self.assertIn("rejected", choice.cuda_error)


class LibraryPathTest(unittest.TestCase):
    def test_present_venv_libraries_are_prepended_once(self):
        with tempfile.TemporaryDirectory() as root:
            site = Path(root)
            for name in ("cublas", "cudnn"):
                (site / "nvidia" / name / "lib").mkdir(parents=True)
            dirs = device.cuda_library_dirs([site])
            self.assertEqual(dirs, [str(site / "nvidia" / "cublas" / "lib"),
                                    str(site / "nvidia" / "cudnn" / "lib")])
            path = device.library_path(dirs, "/usr/lib/other")
            self.assertEqual(path.split(":"), dirs + ["/usr/lib/other"])
            self.assertEqual(device.library_path(dirs, path), path)

    def test_no_venv_libraries_leave_the_path_alone(self):
        with tempfile.TemporaryDirectory() as root:
            self.assertEqual(device.cuda_library_dirs([Path(root)]), [])
        self.assertEqual(device.library_path([], ""), "")


class MissingLibraryWarningTest(unittest.TestCase):
    def test_warning_names_each_missing_library_and_its_package(self):
        missing = device.missing_libraries(lambda name: name == "libcudnn.so.9")
        warning = device.missing_library_warning(missing)
        self.assertIn("libcublas.so.12", warning)
        self.assertIn("nvidia-cublas-cu12", warning)
        self.assertIn("libnvrtc.so.12", warning)
        self.assertIn("nvidia-cuda-nvrtc-cu12", warning)
        self.assertNotIn("libcudnn.so.9", warning)

    def test_no_warning_when_every_library_loads(self):
        self.assertIsNone(device.missing_library_warning(device.missing_libraries(lambda _: True)))


if __name__ == "__main__":
    unittest.main()
