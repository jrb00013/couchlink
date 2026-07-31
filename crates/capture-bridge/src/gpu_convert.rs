//! BGRA -> NV12 on the GPU, using the same D3D11 device the capture runs on.
//!
//! The CPU path reads the captured surface back into system memory, converts a
//! million pixels on the CPU, then hands the result to the encoder, which uploads
//! it to the GPU again. The pixels make two pointless trips across the bus and pay
//! for a colour conversion that fixed-function video hardware does for free.
//!
//! Here the captured texture never leaves the GPU: a D3D11 video processor converts
//! it into an NV12 texture, and the encoder is given that texture directly.
//!
//! Everything in here is best-effort. A machine without a video processor, or a
//! device that refuses these formats, falls back to the CPU path — which is slower
//! but works everywhere.

use anyhow::{Context as _, Result};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_SAMPLE_DESC,
};

/// How many NV12 targets to rotate through. The encoder holds a submitted texture
/// until it is done with it, so writing straight back into the same one would race
/// with an encode still in flight.
const POOL_SIZE: usize = 3;

pub struct GpuConverter {
    device: ID3D11Device,
    video_device: ID3D11VideoDevice,
    video_context: ID3D11VideoContext,
    processor: ID3D11VideoProcessor,
    enumerator: ID3D11VideoProcessorEnumerator,
    pool: Vec<ID3D11Texture2D>,
    next: usize,
    src: (u32, u32),
    dst: (u32, u32),
}

// SAFETY: the D3D11 device is created without D3D11_CREATE_DEVICE_SINGLETHREADED, so
// the runtime serialises access internally and its objects may be used from any
// thread. The converter is built on the capture thread and used from the encoder
// thread, which is exactly what that guarantee covers.
unsafe impl Send for GpuConverter {}

impl GpuConverter {
    /// The video processor scales as well as converts, so the downscale to the wire
    /// resolution happens here too — in one fixed-function pass, instead of a CPU
    /// nearest-neighbour loop over a million pixels.
    pub fn new(
        device: &ID3D11Device,
        src_w: u32,
        src_h: u32,
        dst_w: u32,
        dst_h: u32,
    ) -> Result<Self> {
        unsafe {
            let video_device: ID3D11VideoDevice = device
                .cast()
                .context("device has no video support (ID3D11VideoDevice)")?;
            let context = device.GetImmediateContext().context("immediate context")?;
            let video_context: ID3D11VideoContext =
                context.cast().context("context has no video support")?;

            // Progressive, same size in and out: we only want the colour conversion,
            // so any scaling stays where it already happens.
            let desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
                InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                InputWidth: src_w,
                InputHeight: src_h,
                OutputWidth: dst_w,
                OutputHeight: dst_h,
                Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
                ..Default::default()
            };
            let enumerator = video_device
                .CreateVideoProcessorEnumerator(&desc)
                .context("CreateVideoProcessorEnumerator")?;
            let processor = video_device
                .CreateVideoProcessor(&enumerator, 0)
                .context("CreateVideoProcessor")?;

            let mut pool = Vec::with_capacity(POOL_SIZE);
            for _ in 0..POOL_SIZE {
                pool.push(create_nv12_target(device, dst_w, dst_h)?);
            }

            Ok(Self {
                device: device.clone(),
                video_device,
                video_context,
                processor,
                enumerator,
                pool,
                next: 0,
                src: (src_w, src_h),
                dst: (dst_w, dst_h),
            })
        }
    }

    /// (source, destination) sizes this converter was built for.
    pub const fn dimensions(&self) -> ((u32, u32), (u32, u32)) {
        (self.src, self.dst)
    }

    /// Convert a captured BGRA texture into NV12, entirely on the GPU. The returned
    /// texture belongs to the pool and stays valid until it comes round again.
    pub fn to_nv12(&mut self, src: &ID3D11Texture2D) -> Result<ID3D11Texture2D> {
        unsafe {
            let dst = self.pool[self.next].clone();
            self.next = (self.next + 1) % self.pool.len();

            let in_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                FourCC: 0,
                ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VPIV {
                        MipSlice: 0,
                        ArraySlice: 0,
                    },
                },
            };
            let mut input_view: Option<ID3D11VideoProcessorInputView> = None;
            self.video_device
                .CreateVideoProcessorInputView(src, &self.enumerator, &in_desc, Some(&mut input_view))
                .context("CreateVideoProcessorInputView")?;

            let out_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
                },
            };
            let mut output_view: Option<ID3D11VideoProcessorOutputView> = None;
            self.video_device
                .CreateVideoProcessorOutputView(
                    &dst,
                    &self.enumerator,
                    &out_desc,
                    Some(&mut output_view),
                )
                .context("CreateVideoProcessorOutputView")?;

            let streams = [D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: true.into(),
                OutputIndex: 0,
                InputFrameOrField: 0,
                PastFrames: 0,
                FutureFrames: 0,
                pInputSurface: std::mem::ManuallyDrop::new(input_view),
                ..Default::default()
            }];
            let result = self.video_context.VideoProcessorBlt(
                &self.processor,
                output_view.as_ref().context("no output view")?,
                0,
                &streams,
            );
            // The struct holds a ManuallyDrop interface pointer that we own and must
            // release, whether or not the blit succeeded.
            for stream in streams {
                drop(std::mem::ManuallyDrop::into_inner(stream.pInputSurface));
            }
            result.context("VideoProcessorBlt")?;

            Ok(dst)
        }
    }

    #[allow(dead_code)]
    pub const fn device(&self) -> &ID3D11Device {
        &self.device
    }
}

/// NV12 render target the video processor can write and the encoder can read.
unsafe fn create_nv12_target(
    device: &ID3D11Device,
    width: u32,
    height: u32,
) -> Result<ID3D11Texture2D> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_NV12,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        // RENDER_TARGET so the video processor can output into it; the encoder reads
        // it as a plain shader resource.
        BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut texture: Option<ID3D11Texture2D> = None;
    device
        .CreateTexture2D(&desc, None, Some(&mut texture))
        .context("CreateTexture2D(NV12)")?;
    texture.context("CreateTexture2D returned nothing")
}

/// Is this texture in the format the converter expects as input?
pub fn is_bgra(texture: &ID3D11Texture2D) -> bool {
    unsafe {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        texture.GetDesc(&mut desc);
        desc.Format == DXGI_FORMAT_B8G8R8A8_UNORM
    }
}

/// Width/height of a texture, so the caller can size the encoder from the surface
/// itself rather than assuming.
pub fn texture_size(texture: &ID3D11Texture2D) -> (u32, u32, DXGI_FORMAT) {
    unsafe {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        texture.GetDesc(&mut desc);
        (desc.Width, desc.Height, desc.Format)
    }
}

use windows::core::Interface;

/// A private NV12 texture the encoder thread owns outright, used to hold the last
/// submitted frame so a static screen can keep the cadence going.
///
/// The converter's pool cannot be used for this: the capture thread rotates through
/// it on every WGC frame regardless of what the encoder is doing, so a pooled
/// texture held for replay gets recycled and overwritten mid-encode. Only a texture
/// written by the same thread that submits it is safe to re-submit.
pub struct ReplayTarget {
    /// Two textures, alternating. A single one races itself: the copy that refreshes
    /// it can land while the encoder is still reading the previous submission of the
    /// same surface, which corrupts the frame in flight.
    textures: [ID3D11Texture2D; 2],
    current: usize,
    context: ID3D11DeviceContext,
    dims: (u32, u32),
}

// SAFETY: same multithreaded-device reasoning as GpuConverter; this is created and
// used entirely on the encoder thread.
unsafe impl Send for ReplayTarget {}

impl ReplayTarget {
    pub fn new(device: &ID3D11Device, width: u32, height: u32) -> Result<Self> {
        unsafe {
            let textures = [
                create_nv12_target(device, width, height)?,
                create_nv12_target(device, width, height)?,
            ];
            let context = device.GetImmediateContext().context("immediate context")?;
            Ok(Self {
                textures,
                current: 0,
                context,
                dims: (width, height),
            })
        }
    }

    pub const fn dimensions(&self) -> (u32, u32) {
        self.dims
    }

    /// Keep a copy of a frame that has just been submitted, into whichever buffer is
    /// not the one most recently handed to the encoder.
    pub fn store(&mut self, src: &ID3D11Texture2D) {
        let next = 1 - self.current;
        unsafe { self.context.CopyResource(&self.textures[next], src) };
        self.current = next;
    }

    /// The most recently stored frame.
    pub const fn texture(&self) -> &ID3D11Texture2D {
        &self.textures[self.current]
    }
}
