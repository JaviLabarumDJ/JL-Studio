"""User-facing version, independent of the monotonic signed updater build."""
from pathlib import Path
import re


def display_version():
    label = Path(__file__).with_name('product-version.txt').read_text(encoding='ascii').strip()
    if not re.fullmatch(r'\d+\.\d+(?:\.\d+)?', label):
        raise ValueError(f'Invalid ML Studio version {label!r}')
    return label
