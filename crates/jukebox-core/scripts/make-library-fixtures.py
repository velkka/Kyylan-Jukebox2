#!/usr/bin/env python3
"""Builds the tagged audio files the scanner parity test reads.

Every file is a one-second synthetic tone, tagged to exercise one way music-metadata (the
Electron scanner) and the Rust scanner could disagree: tag priority between ID3v2, ID3v1
and APEv2, multi-value frames, genre codes, dates, pictures, and files with no usable tags.

Needs ffmpeg and mutagen. Run from anywhere:

    python3 crates/jukebox-core/scripts/make-library-fixtures.py

then record what Electron's library code makes of them:

    node crates/jukebox-core/scripts/library-oracle.mjs
"""

import shutil
import subprocess
import sys
import tempfile
from base64 import b64encode
from pathlib import Path

from mutagen.apev2 import APEv2
from mutagen.flac import FLAC, Picture
from mutagen.id3 import (
    APIC, ID3, TALB, TCON, TDRC, TIT2, TPE1, TPE2, TPOS, TRCK, TXXX, TYER, Encoding
)
from mutagen.mp4 import MP4, MP4Cover
from mutagen.oggopus import OggOpus
from mutagen.oggvorbis import OggVorbis
from mutagen.wave import WAVE

OUT = Path(__file__).resolve().parent.parent / "tests" / "fixtures" / "library"
TMP = Path(tempfile.mkdtemp())


def ffmpeg(*args):
    subprocess.run(["ffmpeg", "-v", "error", "-y", *args], check=True)


def tone(dest, *codec):
    ffmpeg("-f", "lavfi", "-i", "sine=frequency=440:duration=1:sample_rate=22050",
           "-ac", "1", *codec, str(dest))


def image(name, fmt):
    path = TMP / name
    ffmpeg("-f", "lavfi", "-i", f"color=c={'red' if fmt == 'mjpeg' else 'blue'}:s=8x8",
           "-frames:v", "1", "-c:v", fmt, str(path))
    return path.read_bytes()


JPEG = image("cover.jpg", "mjpeg")
PNG = image("cover.png", "png")


def mp3(name, xing=True):
    path = OUT / name
    tone(path, "-c:a", "libmp3lame", "-b:a", "32k", "-write_xing", "1" if xing else "0",
         "-id3v2_version", "0", "-write_id3v1", "0")
    return path


def id3v1(path, title=b"", artist=b"", album=b"", year=b"", genre=255, track=None):
    """Appends a raw ID3v1(.1) tag; mutagen only writes v1 alongside v2."""
    def field(value, size):
        return value[:size].ljust(size, b"\0")
    comment = field(b"", 28) + (b"\0" + bytes([track]) if track else b"\0\0")
    tag = b"TAG" + field(title, 30) + field(artist, 30) + field(album, 30) + field(year, 4) \
        + comment + bytes([genre])
    with open(path, "ab") as f:
        f.write(tag)


def flac_picture(data, mime, kind=3):
    pic = Picture()
    pic.type = kind
    pic.mime = mime
    pic.width = pic.height = 8
    pic.depth = 24
    pic.data = data
    return pic


def main():
    if OUT.exists():
        shutil.rmtree(OUT)
    OUT.mkdir(parents=True)

    # ---- MP3 -------------------------------------------------------------------------

    p = mp3("id3v23-full.mp3")
    t = ID3()
    t.add(TIT2(encoding=Encoding.UTF16, text="Full Tags"))
    t.add(TPE1(encoding=Encoding.UTF16, text="Äänitys Band"))
    t.add(TALB(encoding=Encoding.LATIN1, text="Löyly Sessions"))
    t.add(TPE2(encoding=Encoding.UTF16, text="Various Artists"))
    t.add(TCON(encoding=Encoding.LATIN1, text="Rock"))
    t.add(TRCK(encoding=Encoding.LATIN1, text="3/12"))
    t.add(TPOS(encoding=Encoding.LATIN1, text="1/2"))
    t.add(TYER(encoding=Encoding.LATIN1, text="1999"))
    t.add(APIC(encoding=Encoding.LATIN1, mime="image/jpeg", type=3, desc="", data=JPEG))
    t.save(p, v2_version=3)

    p = mp3("id3v24-multi.mp3")
    t = ID3()
    t.add(TIT2(encoding=Encoding.UTF8, text=["First Title", "Second Title"]))
    t.add(TPE1(encoding=Encoding.UTF8, text=["Artist One", "Artist Two", "Artist Three"]))
    t.add(TALB(encoding=Encoding.UTF8, text="  Padded Album  "))
    t.add(TCON(encoding=Encoding.UTF8, text=["Rock", "Pop", "Rock"]))
    t.add(TDRC(encoding=Encoding.UTF8, text="2001-05-02"))
    t.add(TRCK(encoding=Encoding.UTF8, text="07"))
    t.add(APIC(encoding=Encoding.UTF8, mime="image/png", type=4, desc="back", data=PNG))
    t.add(APIC(encoding=Encoding.UTF8, mime="image/jpeg", type=3, desc="front", data=JPEG))
    t.save(p, v2_version=4)

    p = mp3("id3v23-slash-artist.mp3")
    t = ID3()
    t.add(TIT2(encoding=Encoding.LATIN1, text="Highway/Tunnel"))
    t.add(TPE1(encoding=Encoding.LATIN1, text="AC/DC"))
    t.add(TCON(encoding=Encoding.LATIN1, text="(17)(18)Eurodisco"))
    t.save(p, v2_version=3, v23_sep="/")

    p = mp3("id3v23-genre-number.mp3")
    t = ID3()
    t.add(TIT2(encoding=Encoding.LATIN1, text="Numbered Genre"))
    t.add(TCON(encoding=Encoding.LATIN1, text="17"))
    t.add(TYER(encoding=Encoding.LATIN1, text="unknown"))
    t.add(TRCK(encoding=Encoding.LATIN1, text="A1"))
    t.save(p, v2_version=3)

    p = mp3("id3v23-latin1-utf8-bytes.mp3")
    t = ID3()
    # UTF-8 bytes declared as Latin-1: the mis-decoded "LÃ¶yly" already in real databases.
    t.add(TIT2(encoding=Encoding.LATIN1, text="Löyly".encode("utf-8").decode("latin-1")))
    t.save(p, v2_version=3)

    p = mp3("id3v2-and-v1.mp3")
    t = ID3()
    t.add(TIT2(encoding=Encoding.UTF16, text="Long Title From ID3v2 That Is Past Thirty Chars"))
    t.add(TPE1(encoding=Encoding.UTF16, text="V2 Artist"))
    t.save(p, v2_version=3)
    id3v1(p, title=b"Long Title From ID3v2 That Is ", artist=b"V1 Artist", album=b"V1 Only Album",
          year=b"1988", genre=13, track=9)

    p = mp3("id3v1-only.mp3")
    id3v1(p, title=b"Only V1  ", artist=b"Old Tagger", album=b"", year=b"1975", genre=0, track=2)

    p = mp3("ape-and-id3v2.mp3")
    t = ID3()
    t.add(TIT2(encoding=Encoding.UTF16, text="ID3 Title"))
    t.add(TPE1(encoding=Encoding.UTF16, text="ID3 Artist"))
    t.add(TALB(encoding=Encoding.UTF16, text="ID3 Album"))
    t.save(p, v2_version=3)
    ape = APEv2()
    ape["Title"] = "APE Title"
    ape["Artist"] = "APE Artist"
    ape["Year"] = "2010"
    ape.save(p)

    p = mp3("whitespace-title.mp3")
    t = ID3()
    t.add(TIT2(encoding=Encoding.UTF16, text=" \t\ufeff "))
    t.add(TPE1(encoding=Encoding.UTF16, text=" Spaced Artist "))
    t.save(p, v2_version=3)

    mp3("untagged.mp3")
    mp3("no-xing-header.mp3", xing=False)

    p = mp3("txxx-artists.mp3")
    t = ID3()
    t.add(TIT2(encoding=Encoding.UTF8, text="Only Artists Frame"))
    t.add(TXXX(encoding=Encoding.UTF8, desc="ARTISTS", text=["Duo One", "Duo Two"]))
    t.save(p, v2_version=4)

    (OUT / "not-audio.mp3").write_bytes(b"this is not an mp3 file\n" * 40)

    # ---- FLAC ------------------------------------------------------------------------

    p = OUT / "vorbis-comments.flac"
    tone(p, "-c:a", "flac")
    f = FLAC(p)
    f["TITLE"] = ["Take One", "Take Two"]
    f["ARTIST"] = ["Main Artist", "Featured Artist"]
    f["ALBUMARTIST"] = "Album Artist"
    f["ALBUM"] = "FLAC Album"
    f["GENRE"] = ["Jazz", "Fusion"]
    f["DATE"] = "2003-01-01"
    f["TRACKNUMBER"] = "4"
    f["TRACKTOTAL"] = "10"
    f["DISCNUMBER"] = "2/3"
    f.add_picture(flac_picture(PNG, "image/png", kind=0))
    f.add_picture(flac_picture(JPEG, "image/jpeg", kind=3))
    f.save()

    p = OUT / "album-artist-space.flac"
    tone(p, "-c:a", "flac")
    f = FLAC(p)
    f["title"] = "lower-case keys"
    f["album artist"] = "Spaced Key Artist"
    f["year"] = "1994"
    f.save()

    # ---- MP4 -------------------------------------------------------------------------

    p = OUT / "itunes.m4a"
    tone(p, "-c:a", "aac", "-b:a", "32k")
    m = MP4(p)
    m["\xa9nam"] = "iTunes Song"
    m["\xa9ART"] = "iTunes Artist"
    m["\xa9alb"] = "iTunes Album"
    m["aART"] = "iTunes Album Artist"
    m["\xa9gen"] = "Electronic"
    m["\xa9day"] = "2004-03-02T08:00:00Z"
    m["trkn"] = [(5, 11)]
    m["disk"] = [(1, 1)]
    m["covr"] = [MP4Cover(JPEG, imageformat=MP4Cover.FORMAT_JPEG),
                 MP4Cover(PNG, imageformat=MP4Cover.FORMAT_PNG)]
    m.save()

    # ---- Ogg -------------------------------------------------------------------------

    p = OUT / "vorbis.ogg"
    tone(p, "-ac", "2", "-c:a", "vorbis", "-strict", "-2")
    o = OggVorbis(p)
    o["TITLE"] = "Ogg Song"
    o["ARTIST"] = "Ogg Artist"
    o["ALBUM"] = "Ogg Album"
    o["TRACKNUMBER"] = "2/9"
    o["DATE"] = "1985"
    o["METADATA_BLOCK_PICTURE"] = [b64encode(flac_picture(JPEG, "image/jpeg").write()).decode()]
    o.save()

    p = OUT / "vorbis.oga"
    tone(p, "-ac", "2", "-c:a", "vorbis", "-strict", "-2")
    o = OggVorbis(p)
    o["TITLE"] = "Oga Extension"
    o.save()

    p = OUT / "opus.opus"
    tone(p, "-c:a", "libopus", "-b:a", "16k")
    o = OggOpus(p)
    o["TITLE"] = "Opus Song"
    o["ARTIST"] = "Opus Artist"
    o["GENRE"] = "Ambient"
    o.save()

    # ---- WAV and raw AAC -------------------------------------------------------------

    p = OUT / "id3-chunk.wav"
    tone(p, "-c:a", "pcm_s16le")
    w = WAVE(p)
    w.add_tags()
    w.tags.add(TIT2(encoding=Encoding.UTF8, text="Wave Song"))
    w.tags.add(TPE1(encoding=Encoding.UTF8, text="Wave Artist"))
    w.save()

    p = OUT / "riff-info.wav"
    tone(p, "-c:a", "pcm_s16le", "-metadata", "title=Info Title", "-metadata", "artist=Info Artist",
         "-metadata", "album=Info Album", "-metadata", "genre=Folk", "-metadata", "date=1970")

    raw = TMP / "raw.aac"
    tone(raw, "-c:a", "aac", "-b:a", "32k", "-f", "adts")
    p = OUT / "adts-with-id3.aac"
    t = ID3()
    t.add(TIT2(encoding=Encoding.UTF8, text="ADTS Song"))
    t.add(TPE1(encoding=Encoding.UTF8, text="ADTS Artist"))
    head = TMP / "head.id3"
    head.write_bytes(b"")
    t.save(head, v2_version=4, padding=lambda info: 0)
    p.write_bytes(head.read_bytes() + raw.read_bytes())

    # ---- Not scanned -----------------------------------------------------------------

    (OUT / "cover.jpg").write_bytes(JPEG)
    (OUT / ".hidden.mp3").write_bytes((OUT / "untagged.mp3").read_bytes())

    # A subfolder, walked at its place in byte order: after ".hidden.mp3", before "adts…".
    (OUT / "Disc 2").mkdir()
    (OUT / "Disc 2" / "bonus.mp3").write_bytes((OUT / "id3v23-full.mp3").read_bytes())

    shutil.rmtree(TMP)
    names = sorted(p.name for p in OUT.iterdir())
    print(f"{len(names)} files in {OUT}")


if __name__ == "__main__":
    sys.exit(main())
