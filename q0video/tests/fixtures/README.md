the three `tone` files contain a generated 440 hz sine, 0.25 seconds at
44.1 khz, mono. they contain no third party recording.

generated with ffmpeg's `sine=frequency=440:sample_rate=44100:duration=0.25`
source and the libmp3lame, libvorbis and flac encoders. compressed audio tests
use checked in fixtures and do not require ffmpeg or an audio device.
