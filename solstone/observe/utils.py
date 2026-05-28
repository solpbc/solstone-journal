# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Utilities for working with media files and shared observer helpers."""

import datetime
import hashlib
import json
import logging
import os
import random
import re
import time
from pathlib import Path

import numpy as np
import soundfile as sf

from solstone.think.media import AUDIO_EXTENSIONS as _AUDIO_EXTENSIONS
from solstone.think.media import VIDEO_EXTENSIONS as _VIDEO_EXTENSIONS
from solstone.think.utils import day_path

logger = logging.getLogger(__name__)

# Standard sample rate for audio processing
SAMPLE_RATE = 16000

VIDEO_EXTENSIONS = tuple(_VIDEO_EXTENSIONS)
AUDIO_EXTENSIONS = tuple(_AUDIO_EXTENSIONS)


def audio_to_flac_bytes(audio: np.ndarray, sample_rate: int) -> bytes:
    """Convert audio buffer to FLAC bytes.

    Args:
        audio: Audio waveform (mono, float32)
        sample_rate: Sample rate in Hz

    Returns:
        FLAC-encoded audio bytes
    """
    import io

    # Convert to int16 for FLAC encoding
    audio_int16 = (np.clip(audio, -1.0, 1.0) * 32767).astype(np.int16)

    buf = io.BytesIO()
    sf.write(buf, audio_int16, sample_rate, format="FLAC")
    return buf.getvalue()


def load_audio(raw_path: Path, sample_rate: int = SAMPLE_RATE) -> np.ndarray:
    """Load audio file into a numpy buffer using PyAV.

    All supported formats are decoded through PyAV. For M4A files from sck-cli
    (which contain two mono streams: track 0 =
    system audio, track 1 = microphone), all streams are decoded and mixed
    together. Other formats (.flac, .mp3, .ogg, .opus, .wav) decode the first
    audio stream only.

    Parameters
    ----------
    raw_path : Path
        Path to audio file (.flac, .ogg, .opus, .wav, .mp3, or .m4a)
    sample_rate : int
        Target sample rate (default: 16000)

    Returns
    -------
    np.ndarray
        Audio waveform as float32 mono at the target sample rate

    Raises
    ------
    ValueError
        If no audio streams found in M4A file
    RuntimeError
        If PyAV fails to decode a non-M4A file
    """
    if raw_path.suffix.lower() != ".m4a":
        import av

        suffix = raw_path.suffix.lower()
        try:
            with av.open(str(raw_path)) as container:
                streams = list(container.streams.audio)
                if not streams:
                    raise RuntimeError(
                        f"failed to decode {raw_path} ({suffix}): "
                        "no audio streams found"
                    )
                stream = streams[0]
                resampler = av.audio.resampler.AudioResampler(
                    format="flt", layout="mono", rate=sample_rate
                )
                chunks = []
                for frame in container.decode(stream):
                    for out_frame in resampler.resample(frame):
                        arr = out_frame.to_ndarray()
                        chunks.append(arr)
                for out_frame in resampler.resample(None):
                    arr = out_frame.to_ndarray()
                    chunks.append(arr)
                if not chunks:
                    raise RuntimeError(
                        f"failed to decode {raw_path} ({suffix}): no audio data decoded"
                    )
        except av.error.FFmpegError as e:
            raise RuntimeError(f"failed to decode {raw_path} ({suffix}): {e}") from e

        combined = np.concatenate(chunks, axis=1).flatten()
        return combined.astype(np.float32)

    import av

    logger.info(f"Loading m4a with stream mixing: {raw_path}")

    # First pass: count streams
    container = av.open(str(raw_path))
    num_streams = len(list(container.streams.audio))
    container.close()

    if num_streams == 0:
        raise ValueError(f"No audio streams found in {raw_path}")

    # Decode each stream separately (PyAV requires fresh container per stream)
    # sck-cli produces: track 0 = system audio, track 1 = microphone
    stream_data = []
    for stream_idx in range(num_streams):
        container = av.open(str(raw_path))
        stream = list(container.streams.audio)[stream_idx]

        resampler = av.audio.resampler.AudioResampler(
            format="flt", layout="mono", rate=sample_rate
        )
        chunks = []
        for frame in container.decode(stream):
            for out_frame in resampler.resample(frame):
                arr = out_frame.to_ndarray()
                chunks.append(arr)

        container.close()

        if chunks:
            combined = np.concatenate(chunks, axis=1).flatten()
            stream_data.append(combined)
            logger.info(
                f"  Stream {stream_idx}: {len(combined)} samples "
                f"({len(combined) / sample_rate:.1f}s)"
            )

    if not stream_data:
        raise ValueError(f"No audio data decoded from {raw_path}")

    # Mix all streams together
    if len(stream_data) == 1:
        mixed = stream_data[0]
    else:
        # Pad shorter streams to match longest
        max_len = max(len(s) for s in stream_data)
        padded = []
        for s in stream_data:
            if len(s) < max_len:
                s = np.pad(s, (0, max_len - len(s)), mode="constant")
            padded.append(s)
        # Average all streams
        mixed = np.mean(padded, axis=0)
        logger.info(f"  Mixed {len(stream_data)} streams -> {len(mixed)} samples")

    return mixed.astype(np.float32)


def get_segment_key(media_path: Path) -> str | None:
    """
    Extract segment key from a media file path.

    For the new model, files are always in segment directories (HHMMSS_LEN/).
    The segment key is the parent directory name.

    Parameters
    ----------
    media_path : Path
        Path to media file (audio or video)

    Returns
    -------
    str or None
        Segment key in HHMMSS_LEN format, or None if not found

    Examples
    --------
    >>> get_segment_key(Path("/journal/20250101/143022_300/audio.flac"))
    "143022_300"
    >>> get_segment_key(Path("/journal/20250101/random.txt"))
    None
    """
    from solstone.think.utils import segment_key

    # Segment key is the parent directory name
    return segment_key(media_path.parent.name)


def segment_and_suffix(media_path: Path) -> tuple[str, str]:
    """
    Extract segment key and descriptive suffix from a media file path.

    For the new model, files are always in segment directories.
    The segment key is the parent directory name, suffix is the file stem.

    Parameters
    ----------
    media_path : Path
        Path to media file (audio or video) in a segment directory

    Returns
    -------
    tuple[str, str]
        (segment_key, suffix) - e.g., ("143022_300", "audio")

    Raises
    ------
    ValueError
        If the parent directory is not a valid segment

    Examples
    --------
    >>> segment_and_suffix(Path("/journal/20250101/143022_300/audio.flac"))
    ("143022_300", "audio")
    >>> segment_and_suffix(Path("/journal/20250101/143022_300/center_DP-3_screen.webm"))
    ("143022_300", "center_DP-3_screen")
    """
    from solstone.think.utils import segment_key

    # Segment key is the parent directory name
    segment = segment_key(media_path.parent.name)
    if segment is None:
        raise ValueError(
            f"File not in segment directory: {media_path} "
            f"(parent {media_path.parent.name} is not HHMMSS_LEN format)"
        )

    # Suffix is the file stem
    return segment, media_path.stem


def parse_screen_filename(filename: str) -> tuple[str, str]:
    """
    Parse position and connector/displayID from a per-monitor screen filename.

    Files are in segment directories with format: position_connector_screen.ext
    Works with both GNOME connector IDs (e.g., "DP-3") and macOS displayIDs (e.g., "1").

    Parameters
    ----------
    filename : str
        Filename stem (without extension), e.g.:
        - "center_DP-3_screen" (GNOME)
        - "center_1_screen" (macOS)

    Returns
    -------
    tuple[str, str]
        (position, connector) tuple, e.g., ("center", "DP-3") or ("center", "1")
        Returns ("unknown", "unknown") if pattern doesn't match

    Examples
    --------
    >>> parse_screen_filename("center_DP-3_screen")
    ("center", "DP-3")
    >>> parse_screen_filename("center_1_screen")
    ("center", "1")
    >>> parse_screen_filename("left_HDMI-1_screen")
    ("left", "HDMI-1")
    """
    # Pattern: position_connector_screen
    # Connector can be alphanumeric with hyphens (GNOME: DP-3) or just numeric (macOS: 1)
    match = re.match(r"^([a-z-]+)_([A-Za-z0-9-]+)_screen$", filename)
    if match:
        return match.group(1), match.group(2)

    return "unknown", "unknown"


def assign_monitor_positions(monitors: list[dict]) -> list[dict]:
    """
    Assign position labels to monitors based on relative positions.

    Uses pairwise comparison to determine positions. Vertical labels (top/bottom)
    are only assigned when monitors actually overlap horizontally, avoiding
    phantom relationships from offset monitors.

    Parameters
    ----------
    monitors : list[dict]
        List of monitor dicts, each with keys:
        - id: Monitor identifier (e.g., "DP-3", "HDMI-1")
        - box: [x1, y1, x2, y2] coordinates

    Returns
    -------
    list[dict]
        Same monitors with "position" key added to each:
        - "center": No monitors on both sides
        - "left"/"right": Horizontal position
        - "top"/"bottom": Vertical position (only with horizontal overlap)
        - "left-top", "right-bottom", etc.: Corner positions

    Examples
    --------
    >>> monitors = [
    ...     {"id": "DP-1", "box": [0, 0, 1920, 1080]},
    ...     {"id": "DP-2", "box": [1920, 0, 3840, 1080]},
    ... ]
    >>> result = assign_monitor_positions(monitors)
    >>> result[0]["position"]
    'left'
    >>> result[1]["position"]
    'right'
    """
    if not monitors:
        return []

    if len(monitors) == 1:
        monitors[0]["position"] = "center"
        return monitors

    # Tolerance for center classification
    epsilon = 1

    for m in monitors:
        x1, y1, x2, y2 = m["box"]
        center_x = (x1 + x2) / 2
        center_y = (y1 + y2) / 2

        has_left = False
        has_right = False
        has_above = False
        has_below = False

        for other in monitors:
            if other is m:
                continue

            ox1, oy1, ox2, oy2 = other["box"]
            other_center_x = (ox1 + ox2) / 2
            other_center_y = (oy1 + oy2) / 2

            # Horizontal relationship (always check)
            if other_center_x < center_x - epsilon:
                has_left = True
            elif other_center_x > center_x + epsilon:
                has_right = True

            # Vertical relationship only if horizontal overlap exists
            # Overlap means ranges intersect (not just touch)
            h_overlap = (x1 < ox2) and (x2 > ox1)
            if h_overlap:
                if other_center_y < center_y - epsilon:
                    has_above = True
                elif other_center_y > center_y + epsilon:
                    has_below = True

        # Determine horizontal label
        if has_left and has_right:
            h_pos = "center"
        elif has_left:
            h_pos = "right"
        elif has_right:
            h_pos = "left"
        else:
            h_pos = "center"

        # Determine vertical label (only if monitors above/below with overlap)
        if has_above and has_below:
            v_pos = "middle"
        elif has_above:
            v_pos = "bottom"
        elif has_below:
            v_pos = "top"
        else:
            v_pos = None

        # Combine positions
        if v_pos is None:
            position = h_pos
        elif h_pos == "center":
            position = v_pos
        else:
            position = f"{h_pos}-{v_pos}"

        m["position"] = position

    return monitors


def load_analysis_frames(jsonl_path: Path, *, keep_errors: bool = False) -> list[dict]:
    """
    Load and parse analysis JSONL, with optional error-frame retention.

    The first line is a header with metadata (e.g., {"raw": "path"}).
    Subsequent frames are sorted by frame_id before being returned.

    Parameters
    ----------
    jsonl_path : Path
        Path to analysis JSONL file
    keep_errors : bool, optional
        When True, include error records that have a ``frame_id`` in the
        returned frame list. Defaults to False.

    Returns
    -------
    list[dict]
        List of frame analysis results, with header first and frames sorted by
        frame_id. Error records are excluded unless ``keep_errors`` is True.
    """
    header = None
    frames = []
    try:
        with open(jsonl_path, "r") as f:
            for line_num, line in enumerate(f, 1):
                line = line.strip()
                if not line:
                    continue
                try:
                    frame = json.loads(line)
                    if "error" in frame:
                        if keep_errors and "frame_id" in frame:
                            frames.append(frame)
                        continue

                    # First line without frame_id is the header
                    if "frame_id" not in frame and header is None:
                        header = frame
                    else:
                        frames.append(frame)
                except json.JSONDecodeError as e:
                    logger.warning(
                        f"Invalid JSON at line {line_num} in {jsonl_path}: {e}"
                    )
    except FileNotFoundError:
        logger.error(f"Analysis file not found: {jsonl_path}")
        return []
    except Exception as e:
        logger.error(f"Error reading {jsonl_path}: {e}")
        return []

    # Sort frames by frame_id for sequential video decoding
    frames.sort(key=lambda f: f.get("frame_id", 0))

    # Return header first, then sorted frames
    if header:
        return [header] + frames
    return frames


# -----------------------------------------------------------------------------
# Observer utilities (shared between Linux and macOS observers)
# -----------------------------------------------------------------------------


def get_timestamp_parts(timestamp: float | None = None) -> tuple[str, str]:
    """Get date and time parts from timestamp.

    Args:
        timestamp: Unix timestamp (default: current time)

    Returns:
        Tuple of (date_part, time_part) like ("20250101", "143022")
    """
    if timestamp is None:
        timestamp = time.time()
    dt = datetime.datetime.fromtimestamp(timestamp)
    date_part = dt.strftime("%Y%m%d")
    time_part = dt.strftime("%H%M%S")
    return date_part, time_part


def create_draft_folder(start_at: float, stream: str) -> str:
    """Create a draft folder for the current segment.

    Args:
        start_at: Segment start timestamp (wall-clock time)
        stream: Stream name (e.g., "archon", "import.apple")

    Returns:
        Path to the draft folder (YYYYMMDD/stream/HHMMSS_draft/)
    """
    date_part, time_part = get_timestamp_parts(start_at)
    day_dir = day_path(date_part)

    # Create draft folder: YYYYMMDD/stream/HHMMSS_draft/
    draft_name = f"{time_part}_draft"
    draft_path = str(day_dir / stream / draft_name)
    os.makedirs(draft_path, exist_ok=True)

    return draft_path


# -----------------------------------------------------------------------------
# Segment deconfliction utilities (shared between remote app and transfer)
# -----------------------------------------------------------------------------

# Maximum attempts to find available segment key
MAX_SEGMENT_ATTEMPTS = 100


def _randomize_segment(segment: str) -> str | None:
    """Apply random +/-1 to either time or duration component.

    Internal helper for find_available_segment.

    Args:
        segment: Segment key in HHMMSS_LEN format

    Returns:
        Modified segment key, or None if modification would be invalid
        (crosses midnight in either direction, or duration would be <= 0)
    """
    time_part, duration_str = segment.split("_")
    h = int(time_part[:2])
    m = int(time_part[2:4])
    s = int(time_part[4:6])
    dur = int(duration_str)

    modify_time = random.choice([True, False])
    delta = random.choice([1, -1])

    if modify_time:
        # Modify time component
        total_seconds = h * 3600 + m * 60 + s + delta
        if total_seconds < 0 or total_seconds >= 86400:
            return None  # Would cross midnight
        h = total_seconds // 3600
        m = (total_seconds % 3600) // 60
        s = total_seconds % 60
    else:
        # Modify duration component
        dur = dur + delta
        if dur <= 0:
            return None  # Duration can't be zero or negative

    return f"{h:02d}{m:02d}{s:02d}_{dur}"


def _segment_exists(parent_dir: Path, segment: str) -> bool:
    """Check if segment key is already in use.

    Internal helper for find_available_segment.

    Args:
        parent_dir: Path to stream directory (day/stream/)
        segment: Segment key in HHMMSS_LEN format

    Returns:
        True if segment directory exists
    """
    return (parent_dir / segment).exists()


def find_available_segment(
    parent_dir: Path, segment: str, max_attempts: int = MAX_SEGMENT_ATTEMPTS
) -> str | None:
    """Find an available segment key using random modifications.

    Uses a random walk approach: each attempt randomly modifies either
    the time or duration by +/-1, exploring the space around the original.

    Args:
        parent_dir: Path to stream directory (day/stream/)
        segment: Original segment key in HHMMSS_LEN format
        max_attempts: Maximum modification attempts before giving up

    Returns:
        Available segment key (may be original or modified), or None if
        no available slot found after max_attempts
    """
    # Check if original is available
    if not _segment_exists(parent_dir, segment):
        return segment

    current = segment
    tried = {segment}

    for _ in range(max_attempts):
        modified = _randomize_segment(current)

        if modified is None:
            # Invalid modification (hit boundary), try again from same position
            continue

        current = modified  # Always move to valid position

        if modified in tried:
            continue  # Already checked, don't recheck filesystem

        tried.add(modified)

        if not _segment_exists(parent_dir, modified):
            return modified

    return None  # Exhausted attempts


def compute_file_sha256(file_path: Path) -> str:
    """Compute SHA256 hash of a file.

    Args:
        file_path: Path to the file

    Returns:
        Hex-encoded SHA256 hash
    """
    sha256 = hashlib.sha256()
    with open(file_path, "rb") as f:
        for chunk in iter(lambda: f.read(65536), b""):
            sha256.update(chunk)
    return sha256.hexdigest()


def compute_bytes_sha256(data: bytes) -> str:
    """Compute SHA256 hash of bytes.

    Args:
        data: Bytes to hash

    Returns:
        Hex-encoded SHA256 hash
    """
    return hashlib.sha256(data).hexdigest()
