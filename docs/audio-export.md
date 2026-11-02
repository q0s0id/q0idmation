# audio storage and mp4 export

on windows, q0enc mixes audible clips from the selected timeline into the mp4
as 48 khz stereo aac at 160 kbps. the same mixer used for playback applies clip
start, source offset, gain, mute, balance and fades. a selected export range
starts both media clocks at zero and reads audio from that range in the source.
video and audio are submitted together, one timeline frame at a time, so the
writer does not buffer a whole movie to interleave its streams. timelines with
no audible audio clips keep a video-only mp4.

for movies with audio, video frame reordering is disabled: some hardware
encoders otherwise shift the video presentation timestamps by one frame while
the aac track begins at zero. video-only encoding keeps its existing settings.

the media foundation aac types follow the supported rates and bitrates in
[microsoft's encoder documentation](https://learn.microsoft.com/en-us/windows/win32/medfound/aac-encoder).

audio imported from mp3, ogg/vorbis or flac keeps the original file bytes in
q0v. the audio-only container adds an 80-byte header, without transcoding or
expanding the saved file into pcm. the source sample rate and channel count
are retained; the mixer converts to its 48 khz stereo clock during playback.
wav import and mixed q0v exports use lossless zlib blocks of at most 4096
sample frames instead of saving raw pcm. compression of already decoded music
depends on the recording and does not restore the original mp3's size.

q0v v2 uses the existing 80-byte header and frame index. the previously reserved
u32 at offset 36 selects the audio codec: 0 is legacy signed 16-bit pcm, 1 is
lossless pcm blocks, and 2 embeds the source audio file. codec 1 blocks contain
the decoded byte count and stored byte count as little-endian u32 values,
followed by a zlib stream. the frame index begins after the stored audio data;
its offset determines the encoded audio length. decoded size and duration are
checked separately. original encoded audio is decoded with gapless trimming.

the readers accept v1 and v2. legacy pcm writers and video-only files still
emit v1; compressed audio writes v2. older application builds cannot read the
new audio files, so update both editor and player. previously imported pcm
assets retain their contents; reimport their original compressed files to get
the smaller storage without another lossy encode.

waveform decoding runs on the existing background worker. repeated mixer
chunks share decoded immutable audio, and playback does not reopen and decode
the source for every chunk. decoding is limited to 240 mib per audio asset.
compressed data reduces file size; working playback memory still includes pcm.

validation:

- `cargo test -p q0video --lib --release --locked --offline`
- `cargo test -p q0editor -p q0player --lib --release --locked --offline audio`
- `cargo run -p q0editor --example verify_audio_export --release --locked --offline -- <audio-file> <output-directory>`

the headless example writes the imported q0v, a project with a source offset,
gain, balance, fades and a muted clip, a selected-range mp4 and q0v, and the
q0v's decoded pcm for comparison with an independent mp4 decoder. it creates
no windows and requires no audio device. small checked-in mp3, ogg and flac
fixtures are generated sine waves, with no third party recording.
