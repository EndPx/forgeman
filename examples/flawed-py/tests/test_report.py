import unittest

from report import slugify, top_titles


class ReportTests(unittest.TestCase):
    def test_slugify_uses_hyphens(self):
        self.assertEqual(slugify("Hello World"), "hello-world")

    def test_slugify_strips_and_lowers(self):
        self.assertEqual(slugify("  OK  "), "ok")

    def test_top_titles_orders_by_frequency(self):
        self.assertEqual(
            top_titles(["a", "b", "a", "c", "a", "b"], 2), ["a", "b"]
        )


if __name__ == "__main__":
    unittest.main()
