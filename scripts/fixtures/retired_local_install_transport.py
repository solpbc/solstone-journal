import hashlib
import tarfile

LLAMA_SERVER_PINS = {}


def _safe_extract_tarball():
    return tarfile.open("retired.tar")


def install_local():
    return hashlib.sha256()


def start_bootstrap():
    return acquire_install_lease("local")
