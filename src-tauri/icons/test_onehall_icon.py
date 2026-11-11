"""Regression for the raster source's internal transparent/antialiased inset."""
import importlib.util
import tempfile
import unittest
from pathlib import Path

from PIL import Image, ImageDraw


class MacosBrandGeometry(unittest.TestCase):
    def test_macos_body_fits_the_grid_even_with_source_padding(self):
        script = Path(__file__).with_name("onehall-icon.gen.py")
        spec = importlib.util.spec_from_file_location("onehall_icon", script)
        generator = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(generator)
        for body_alpha in (255, 253):
            with self.subTest(body_alpha=body_alpha):
                with tempfile.TemporaryDirectory() as temporary:
                    root = Path(temporary)
                    generator.ICONS = root / "icons"
                    generator.PUBLIC = root / "public"
                    generator.ICONS.mkdir()
                    generator.PUBLIC.mkdir()
                    generator.SOURCE = root / "source.png"
                    source = Image.new("RGBA", (104, 104), (0, 0, 0, 0))
                    ImageDraw.Draw(source).rectangle((2, 2, 101, 101), fill=(20, 80, 80, body_alpha))
                    source.save(generator.SOURCE)
                    rendered = {}

                    def encode(png, output):
                        # Only the external container encoder is replaced. The real
                        # generator renders its canvas, resamples alpha and writes PNG.
                        output.mkdir(parents=True, exist_ok=True)
                        if png.name == "mac.png":
                            with Image.open(png) as image:
                                rendered["mac"] = image.convert("RGBA")
                        (output / "icon.icns").write_bytes(b"encoder output")

                    generator.tauri_icon = encode
                    generator.main()
                    mac = rendered["mac"]
                    self.assertEqual(mac.size, (1024, 1024))
                    alpha = mac.getchannel("A")
                    width = sum(alpha.crop((0, 512, 1024, 513)).getdata()) / 255
                    height = sum(alpha.crop((512, 0, 513, 1024)).getdata()) / 255
                    self.assertAlmostEqual(width, 824, delta=2)
                    self.assertAlmostEqual(height, 824, delta=2)
                    self.assertEqual(mac.getpixel((512, 512)), (20, 80, 80, 255))
                    for values in (
                        [alpha.getpixel((x, 512)) for x in range(1024)],
                        [alpha.getpixel((512, y)) for y in range(1024)],
                    ):
                        opaque = [i for i, value in enumerate(values) if value > 127]
                        self.assertAlmostEqual(opaque[0], 100, delta=2)
                        self.assertAlmostEqual(1023 - opaque[-1], 100, delta=2)


if __name__ == "__main__":
    unittest.main()
