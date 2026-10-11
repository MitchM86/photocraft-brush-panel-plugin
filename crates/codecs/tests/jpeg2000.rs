//! JPEG 2000 specifics: lossy quality, the bare codestream, CMYK and 16-bit profiles, a large
//! (multi-threaded) image and the resolution box.

mod common;
use common::*;
use photocraft_codecs::*;

fn lossy(q: u8) -> EncodeOptions {
    EncodeOptions { jpeg2000_lossless: false, jpeg2000_quality: q, ..Default::default() }
}

#[test]
fn lossless_is_exact_in_every_layout_and_depth() {
    for layout in ChannelLayout::ALL {
        for sample in [SampleType::U8, SampleType::U16] {
            let img = synth(53, 31, layout, sample, 3, 0.3);
            let back = decode(&encode(&img, Format::Jpeg2000, &EncodeOptions::default()).unwrap()).unwrap();
            assert_eq!((back.layout(), back.sample_type()), (layout, sample));
            assert_eq!(back.data(), img.data(), "{layout:?} {sample:?}");
        }
    }
}

#[test]
fn quality_trades_size_for_fidelity() {
    let img = synth(160, 120, ChannelLayout::Rgb, SampleType::U8, 9, 0.02);
    let lossless = encode(&img, Format::Jpeg2000, &EncodeOptions::default()).unwrap();
    let mut last: Option<(usize, f64)> = None;
    for q in [10u8, 50, 90] {
        let bytes = encode(&img, Format::Jpeg2000, &lossy(q)).unwrap();
        let p = psnr(&img, &decode(&bytes).unwrap());
        assert!(bytes.len() < lossless.len(), "q{q}: {} ≥ lossless {}", bytes.len(), lossless.len());
        if let Some((len, prev)) = last {
            assert!(bytes.len() > len && p > prev, "q{q}: {} bytes {p:.1} dB after {len} bytes {prev:.1} dB", bytes.len());
        }
        last = Some((bytes.len(), p));
    }
    assert!(last.unwrap().1 > 35.0, "q90 PSNR {:.1}", last.unwrap().1);
    assert!(fidelity_warnings_with(&img, Format::Jpeg2000, &lossy(50)).contains(&FidelityWarning::LossyCompression));
    assert!(!fidelity_warnings(&img, Format::Jpeg2000).contains(&FidelityWarning::LossyCompression));
}

#[test]
fn lossy_keeps_alpha_and_cmyk() {
    for layout in [ChannelLayout::Rgba, ChannelLayout::CmykA, ChannelLayout::GrayA] {
        let img = synth(64, 48, layout, SampleType::U16, 4, 0.01);
        let back = decode(&encode(&img, Format::Jpeg2000, &lossy(80)).unwrap()).unwrap();
        assert_eq!((back.layout(), back.sample_type()), (layout, SampleType::U16));
        assert!(psnr(&img, &back) > 30.0, "{layout:?} {:.1}", psnr(&img, &back));
    }
}

#[test]
fn codestream_has_only_the_pixels() {
    let mut img = test_image(ChannelLayout::Rgba, SampleType::U8);
    img.icc = Some(sample_icc(700));
    img.meta.dpi = Some((300.0, 300.0));
    let bytes = encode(&img, Format::Jpeg2000, &EncodeOptions { jpeg2000_codestream: true, ..Default::default() }).unwrap();
    assert!(bytes.starts_with(&[0xFF, 0x4F, 0xFF, 0x51]));
    assert_eq!(detect(&bytes), Some(Format::Jpeg2000));
    let back = decode(&bytes).unwrap();
    // Four components without a channel definition read as RGBA, as OpenJPEG writes them.
    assert_eq!(back.layout(), ChannelLayout::Rgba);
    assert_eq!(back.data(), img.data());
    assert!(back.icc.is_none() && back.meta.dpi.is_none());
}

#[test]
fn cmyk_profile_and_fractional_resolution_survive() {
    let mut img = test_image(ChannelLayout::Cmyk, SampleType::U16);
    let mut icc = sample_icc(2000);
    icc[16..20].copy_from_slice(b"CMYK");
    img.icc = Some(icc);
    img.meta.dpi = Some((72.5, 1200.0));
    let bytes = encode(&img, Format::Jpeg2000, &EncodeOptions::default()).unwrap();
    let back = decode(&bytes).unwrap();
    assert_eq!(back.layout(), ChannelLayout::Cmyk);
    assert_eq!(back.icc, img.icc);
    let (x, y) = back.meta.dpi.unwrap();
    assert!((x - 72.5).abs() < 0.01 && (y - 1200.0).abs() < 0.5, "{x} {y}");
}

#[test]
fn rgb_profile_is_not_attached_to_gray() {
    let mut img = test_image(ChannelLayout::Gray, SampleType::U8);
    let mut icc = sample_icc(600);
    icc[16..20].copy_from_slice(b"RGB ");
    img.icc = Some(icc);
    // The encoder writes whatever it is given; the decoder refuses a profile that cannot
    // describe one gray channel.
    let back = decode(&encode(&img, Format::Jpeg2000, &EncodeOptions::default()).unwrap()).unwrap();
    assert!(back.icc.is_none());
}

#[test]
fn large_image_uses_threads_and_stays_exact() {
    let img = synth(1100, 1000, ChannelLayout::Rgb, SampleType::U8, 11, 0.1);
    let back = decode(&encode(&img, Format::Jpeg2000, &EncodeOptions::default()).unwrap()).unwrap();
    assert_eq!(back.data(), img.data());
    let back = decode(&encode(&img, Format::Jpeg2000, &lossy(60)).unwrap()).unwrap();
    assert!(psnr(&img, &back) > 25.0);
}

#[test]
fn thin_images_roundtrip() {
    for (w, h) in [(1, 1), (1, 40), (40, 1), (2, 3), (129, 2)] {
        let img = synth(w, h, ChannelLayout::Rgba, SampleType::U8, 5, 0.3);
        for opts in [EncodeOptions::default(), lossy(50)] {
            let back = decode(&encode(&img, Format::Jpeg2000, &opts).unwrap()).unwrap();
            assert_eq!(back.dimensions(), (w, h));
            if opts.jpeg2000_lossless {
                assert_eq!(back.data(), img.data(), "{w}x{h}");
            }
        }
    }
}

#[test]
fn truncated_files_fail_cleanly() {
    let bytes = encode(&test_image(ChannelLayout::Rgb, SampleType::U8), Format::Jpeg2000, &EncodeOptions::default()).unwrap();
    for cut in [0, 10, 40, 80, bytes.len() / 2, bytes.len() - 1] {
        let _ = decode_as(Format::Jpeg2000, &bytes[..cut]);
    }
}
