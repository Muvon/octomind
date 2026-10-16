import unittest

from merge_ranges import merge_ranges


class MergeRangesTests(unittest.TestCase):
    def test_empty(self):
        self.assertEqual(merge_ranges([]), [])

    def test_disjoint(self):
        self.assertEqual(merge_ranges([(1, 2), (4, 5)]), [(1, 2), (4, 5)])

    def test_overlapping(self):
        self.assertEqual(merge_ranges([(1, 4), (3, 6)]), [(1, 6)])

    def test_touching_endpoints(self):
        self.assertEqual(merge_ranges([(1, 3), (3, 5)]), [(1, 5)])

    def test_nested(self):
        self.assertEqual(merge_ranges([(1, 10), (2, 3)]), [(1, 10)])

    def test_nested_then_overlapping(self):
        self.assertEqual(merge_ranges([(1, 10), (2, 3), (9, 12)]), [(1, 12)])

    def test_unsorted_input_is_not_changed(self):
        ranges = [(4, 8), (1, 5)]
        self.assertEqual(merge_ranges(ranges), [(1, 8)])
        self.assertEqual(ranges, [(4, 8), (1, 5)])


if __name__ == "__main__":
    unittest.main()
