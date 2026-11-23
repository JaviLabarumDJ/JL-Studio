# Noob CompressorLab DSP

ML 11766 and ML LA2AA use Ely Erin Fox's FET and optical DSP and the shared VU movement from [Noob CompressorLab](https://github.com/Noob-Audio-Engineering/noob-compressorlab/tree/cd50250446b8f4fa59f2fe4668d895adb3a37b63). The vendored FET, photocell and transformer models come from [Noob Electrical Components](https://github.com/Noob-Audio-Engineering/noob-electrical-components/tree/8dda53542e701872d6daf6e77e64986be2cb2286). Both are available under MIT or Apache 2.0. ML Studio uses the MIT option. The original notice is in LICENSE-MIT.

The vendored audio algorithms retain their original code. Import paths point at local modules, which provide the original denormal helper. The optical test that compared the LA-2A and LA-3A dividers retains the LA-2A checks. The host adapters and faceplates are in src/compressorlab.
