//! Session picture processing, applied only to the image sent to the display.

use core::{ffi::c_void, ptr::NonNull};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};

use block2::RcBlock;
use objc2::{rc::Retained, runtime::ProtocolObject};
use objc2_foundation::NSString;
use objc2_metal::{
    MTLCommandBuffer, MTLCommandEncoder, MTLDevice, MTLLoadAction, MTLPixelFormat,
    MTLPrimitiveType, MTLRenderCommandEncoder, MTLRenderPassDescriptor, MTLRenderPipelineState,
    MTLResource, MTLStorageMode, MTLStoreAction, MTLTexture, MTLTextureDescriptor, MTLTextureUsage,
};

use super::present;

/// One coherent snapshot is copied under the lock, once per present.
static SETTINGS: Mutex<Picture> = Mutex::new(Picture::neutral());
static ENABLED: AtomicBool = AtomicBool::new(false);
/// A lease stays exclusive until its command buffer completes, including on encode failure.
static SCRATCH: Mutex<Vec<Scratch>> = Mutex::new(Vec::new());
const MAX_IDLE_SCRATCH_SETS: usize = 3;

/// Values are session-local and never cross the PE/Unix ABI.
#[derive(Clone, Copy)]
pub struct Picture {
    pub sharpen: f32,
    pub exposure: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub temperature: f32,
    pub bloom: f32,
    pub bloom_threshold: f32,
    pub bloom_radius: f32,
    pub fxaa: bool,
}

impl Picture {
    pub const fn neutral() -> Self {
        Self {
            sharpen: 0.0,
            exposure: 0.0,
            contrast: 1.0,
            saturation: 1.0,
            temperature: 0.0,
            bloom: 0.0,
            bloom_threshold: 0.75,
            bloom_radius: 3.0,
            fxaa: false,
        }
    }

    fn active(self) -> bool {
        self.fxaa || self.composite_active()
    }

    fn composite_active(self) -> bool {
        self.sharpen > 0.0
            || self.bloom > 0.0
            || self.exposure.abs() > 0.0
            || (self.contrast - 1.0).abs() > 0.0
            || (self.saturation - 1.0).abs() > 0.0
            || self.temperature.abs() > 0.0
    }

    fn uniforms(self) -> [[f32; 4]; 3] {
        [
            [self.sharpen, self.exposure, self.contrast, self.saturation],
            [
                self.temperature,
                self.bloom,
                self.bloom_threshold,
                self.bloom_radius,
            ],
            [0.0; 4],
        ]
    }
}

pub fn snapshot() -> Picture {
    *SETTINGS.lock().unwrap()
}

/// Clamp the session inputs before publishing a complete frame snapshot.
pub fn update(mut value: Picture) {
    super::interpolation::invalidate();
    fn bounded(value: f32, low: f32, high: f32, default: f32) -> f32 {
        if value.is_finite() {
            value.clamp(low, high)
        } else {
            default
        }
    }
    value.sharpen = bounded(value.sharpen, 0.0, 1.0, 0.0);
    value.exposure = bounded(value.exposure, -2.0, 2.0, 0.0);
    value.contrast = bounded(value.contrast, 0.5, 1.5, 1.0);
    value.saturation = bounded(value.saturation, 0.0, 2.0, 1.0);
    value.temperature = bounded(value.temperature, -1.0, 1.0, 0.0);
    value.bloom = bounded(value.bloom, 0.0, 1.0, 0.0);
    value.bloom_threshold = bounded(value.bloom_threshold, 0.0, 1.0, 0.75);
    value.bloom_radius = bounded(value.bloom_radius, 1.0, 8.0, 3.0);
    *SETTINGS.lock().unwrap() = value;
    ENABLED.store(value.active(), Ordering::Release);
    if !value.active() {
        SCRATCH.lock().unwrap().clear();
    }
}

/// Return an independent processed source, or leave the original present path intact.
pub fn encode(
    cb: &ProtocolObject<dyn MTLCommandBuffer>,
    src: &ProtocolObject<dyn MTLTexture>,
) -> Option<Retained<ProtocolObject<dyn MTLTexture>>> {
    // Neutral settings retain the existing presentation path without taking a lock.
    if !ENABLED.load(Ordering::Acquire) {
        return None;
    }
    let value = snapshot();
    if !value.active() {
        return None;
    }
    let result = encode_active(cb, src, value);
    if result.is_none() {
        mtld3d_shared::log_once_warn!(target: crate::LOG_TARGET,
            "picture: resources or encoding unavailable; presenting the original image");
    }
    result
}

/// Each handle owns exactly one retain. No texture can belong to two leases.
struct Scratch {
    width: usize,
    height: usize,
    intermediates: usize,
    textures: Vec<u64>,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        for handle in &self.textures {
            // SAFETY: each handle was created by into_raw, and this owns its sole canonical retain.
            drop(unsafe { Retained::from_raw(*handle as *mut ProtocolObject<dyn MTLTexture>) });
        }
    }
}

impl Scratch {
    fn acquire(
        device: &ProtocolObject<dyn MTLDevice>,
        src: &ProtocolObject<dyn MTLTexture>,
        value: Picture,
    ) -> Option<Self> {
        let fxaa = value.fxaa && value.composite_active();
        let intermediates = usize::from(fxaa) + 2 * usize::from(value.bloom > 0.0);
        {
            let mut pool = SCRATCH.lock().unwrap();
            if let Some(index) = pool.iter().position(|s| {
                s.width == src.width()
                    && s.height == src.height()
                    && s.intermediates == intermediates
            }) {
                return Some(pool.swap_remove(index));
            }
            // Discard idle geometries on resize; in-flight leases retire independently.
            pool.clear();
        }
        let mut scratch = Self {
            width: src.width(),
            height: src.height(),
            intermediates,
            textures: Vec::new(),
        };
        scratch.add(device, scratch.width, scratch.height, MTLPixelFormat::BGRA8Unorm)?;
        if fxaa {
            scratch.add(device, scratch.width, scratch.height, MTLPixelFormat::BGRA8Unorm)?;
        }
        if value.bloom > 0.0 {
            for _ in 0..2 {
                scratch.add(
                    device,
                    scratch.width.div_ceil(4),
                    scratch.height.div_ceil(4),
                    MTLPixelFormat::RGBA16Float,
                )?;
            }
        }
        Some(scratch)
    }

    fn add(
        &mut self,
        device: &ProtocolObject<dyn MTLDevice>,
        width: usize,
        height: usize,
        format: MTLPixelFormat,
    ) -> Option<()> {
        // SAFETY: positive dimensions come from the source texture; no mip levels are requested.
        let desc = unsafe {
            MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
                format, width, height, false,
            )
        };
        desc.setStorageMode(MTLStorageMode::Private);
        desc.setUsage(MTLTextureUsage::ShaderRead | MTLTextureUsage::RenderTarget);
        let texture = device.newTextureWithDescriptor(&desc)?;
        texture.setLabel(Some(&NSString::from_str("mtld3d-picture-scratch")));
        self.textures.push(Retained::into_raw(texture) as u64);
        Some(())
    }

    fn retire(self, cb: &ProtocolObject<dyn MTLCommandBuffer>) {
        let owned = Mutex::new(Some(self));
        let handler = RcBlock::new(move |_cb: NonNull<ProtocolObject<dyn MTLCommandBuffer>>| {
            if let Some(scratch) = owned.lock().unwrap().take() {
                let mut pool = SCRATCH.lock().unwrap();
                if ENABLED.load(Ordering::Acquire) && pool.len() < MAX_IDLE_SCRATCH_SETS {
                    pool.push(scratch);
                }
            }
        });
        // SAFETY: Metal copies the block and invokes it on completion; the block owns the lease.
        unsafe { cb.addCompletedHandler(RcBlock::as_ptr(&handler)) };
    }
}

fn encode_active(
    cb: &ProtocolObject<dyn MTLCommandBuffer>,
    src: &ProtocolObject<dyn MTLTexture>,
    value: Picture,
) -> Option<Retained<ProtocolObject<dyn MTLTexture>>> {
    let device = cb.device();
    let pipelines = present::ensure_picture_resources(&device)?;
    let scratch = Scratch::acquire(&device, src, value)?;
    let textures: Vec<_> = scratch.textures.iter().map(|handle| {
        // SAFETY: the lease owns a retain on every handle through completion.
        unsafe { Retained::retain(*handle as *mut ProtocolObject<dyn MTLTexture>) }
            .expect("owned texture")
    }).collect();
    // Register before the first encoder: partially encoded passes must keep their lease, too.
    scratch.retire(cb);
    let uniforms = value.uniforms();
    let mut source = src;
    if value.fxaa {
        let output = usize::from(value.composite_active());
        pass(cb, Pass { src, dst: &textures[output], bloom: src, pipeline: pipelines.picture_fxaa, uniforms })?;
        if !value.composite_active() {
            return Some(textures[output].clone());
        }
        source = &textures[output];
    }
    let bloom = if value.bloom > 0.0 {
        let index = 1 + usize::from(value.fxaa);
        let small = &textures[index];
        let blur = &textures[index + 1];
        pass(cb, Pass { src: source, dst: small, bloom: source, pipeline: pipelines.picture_extract, uniforms })?;
        let mut horizontal = uniforms;
        horizontal[2][0] = 1.0;
        pass(cb, Pass { src: small, dst: blur, bloom: small, pipeline: pipelines.picture_blur, uniforms: horizontal })?;
        pass(cb, Pass { src: blur, dst: small, bloom: blur, pipeline: pipelines.picture_blur, uniforms })?;
        &**small
    } else {
        source
    };
    pass(cb, Pass { src: source, dst: &textures[0], bloom, pipeline: pipelines.picture_composite, uniforms })?;
    Some(textures[0].clone())
}

struct Pass<'a> {
    src: &'a ProtocolObject<dyn MTLTexture>,
    dst: &'a ProtocolObject<dyn MTLTexture>,
    bloom: &'a ProtocolObject<dyn MTLTexture>,
    pipeline: u64,
    uniforms: [[f32; 4]; 3],
}

fn pass(cb: &ProtocolObject<dyn MTLCommandBuffer>, args: Pass<'_>) -> Option<()> {
    // SAFETY: the present pipeline cache owns this pipeline for process lifetime.
    let pipeline = unsafe {
        Retained::retain(args.pipeline as *mut ProtocolObject<dyn MTLRenderPipelineState>)
    }?;
    let descriptor = MTLRenderPassDescriptor::new();
    // SAFETY: Metal always provides color attachment slot zero.
    let color = unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
    color.setTexture(Some(args.dst));
    color.setLoadAction(MTLLoadAction::DontCare);
    color.setStoreAction(MTLStoreAction::Store);
    let encoder = cb.renderCommandEncoderWithDescriptor(&descriptor)?;
    encoder.setLabel(Some(&NSString::from_str("mtld3d-picture")));
    encoder.setRenderPipelineState(&pipeline);
    let bytes = NonNull::from(&args.uniforms).cast::<c_void>();
    // SAFETY: all textures are retained through completion; setBytes copies the entire uniform block.
    unsafe {
        encoder.setFragmentTexture_atIndex(Some(args.src), 0);
        encoder.setFragmentTexture_atIndex(Some(args.bloom), 1);
        encoder.setFragmentBytes_length_atIndex(bytes, core::mem::size_of_val(&args.uniforms), 0);
        encoder.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::Triangle, 0, 3);
    }
    encoder.endEncoding();
    Some(())
}
