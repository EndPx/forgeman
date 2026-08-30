"""Deliberately flawed reporting utilities — ForgeMan evaluation case #3.

Planted defects:
1. slugify() joins words with underscores (a test fails).
2. top_titles() rescans and re-counts the whole list for every slot (O(n^2)).
"""


def slugify(title):
    # BUG: underscores instead of hyphens.
    return title.strip().lower().replace(" ", "_")


def top_titles(titles, n):
    result = []
    working = list(titles)
    for _ in range(n):
        best = None
        best_count = 0
        for candidate in working:
            count = working.count(candidate)
            if count > best_count:
                best = candidate
                best_count = count
        if best is None:
            break
        result.append(best)
        working = [t for t in working if t != best]
    return result
