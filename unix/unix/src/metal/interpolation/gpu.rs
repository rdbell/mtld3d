//! Bounded image-derived motion and MetalFX resources for the SDR experiment.

use core::ptr::NonNull;
use std::sync::Mutex;

use block2::RcBlock;
use objc2::{rc::Retained, runtime::ProtocolObject};
use objc2_foundation::NSString;
use objc2_metal::{
    MTLCommandBuffer, MTLCommandEncoder, MTLDevice, MTLLibrary, MTLLoadAction, MTLPixelFormat,
    MTLPrimitiveType, MTLRenderCommandEncoder, MTLRenderPassDescriptor, MTLRenderPipelineState,
    MTLResource, MTLStorageMode, MTLStoreAction, MTLTexture, MTLTextureDescriptor, MTLTextureUsage,
};
use objc2_metal_fx::{
    MTLFXFrameInterpolator, MTLFXFrameInterpolatorBase, MTLFXFrameInterpolatorDescriptor,
};

use super::super::{command, present};

pub struct Resources {
    interpolator: Retained<ProtocolObject<dyn MTLFXFrameInterpolator>>,
    colors: [Retained<ProtocolObject<dyn MTLTexture>>; 2],
    output: Retained<ProtocolObject<dyn MTLTexture>>,
    depth: Retained<ProtocolObject<dyn MTLTexture>>,
    motion: Retained<ProtocolObject<dyn MTLTexture>>,
    grid: Retained<ProtocolObject<dyn MTLTexture>>,
    search: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    expand: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    plane: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    index: usize,
}

/// Both images have identical geometry, format and presentation filtering.
pub struct Images {
    pub real: Retained<ProtocolObject<dyn MTLTexture>>,
    pub generated: Retained<ProtocolObject<dyn MTLTexture>>,
}

// SAFETY: only Metal objects, with no AppKit affinity. The owner serializes all
// property changes and encoding under its mutex and uses exactly one Metal queue.
unsafe impl Send for Resources {}

impl Resources {
    pub fn new(
        device: &ProtocolObject<dyn MTLDevice>,
        size: (usize, usize),
    ) -> Result<Self, &'static str> {
        if !objc2::available!(macos = 26.0) {
            return Err("MetalFX frame interpolation requires macOS 26.");
        }
        // SAFETY: the availability guard precedes every reference to the macOS 26 class.
        if !unsafe { MTLFXFrameInterpolatorDescriptor::supportsDevice(device) } {
            return Err("This device does not support MetalFX frame interpolation.");
        }
        // SAFETY: the class is available; the descriptor is local to this call.
        let descriptor = unsafe { MTLFXFrameInterpolatorDescriptor::new() };
        // SAFETY: color and output resources below use this exact format.
        unsafe { descriptor.setColorTextureFormat(MTLPixelFormat::BGRA8Unorm) };
        // SAFETY: output is a private BGRA8 texture matching the descriptor.
        unsafe { descriptor.setOutputTextureFormat(MTLPixelFormat::BGRA8Unorm) };
        // SAFETY: the synthetic plane is stored in a matching R32Float texture.
        unsafe { descriptor.setDepthTextureFormat(MTLPixelFormat::R32Float) };
        // SAFETY: motion below has two half-float pixel-displacement channels.
        unsafe { descriptor.setMotionTextureFormat(MTLPixelFormat::RG16Float) };
        // SAFETY: size is clamped and nonzero by the caller.
        unsafe { descriptor.setInputWidth(size.0) };
        // SAFETY: same validated size.
        unsafe { descriptor.setInputHeight(size.1) };
        // SAFETY: no temporal scaler is attached; colors and output share this size.
        unsafe { descriptor.setOutputWidth(size.0) };
        // SAFETY: same validated size.
        unsafe { descriptor.setOutputHeight(size.1) };
        // SAFETY: descriptor and device are live and local; failure is a normal fallback.
        let interpolator = unsafe { descriptor.newFrameInterpolatorWithDevice(device) }
            .ok_or("MetalFX declined the interpolation descriptor.")?;
        // SAFETY: these immutable usage requirements belong to this live interpolator.
        let color_usage = unsafe { interpolator.colorTextureUsage() };
        // SAFETY: same instance and immutable property.
        let depth_usage = unsafe { interpolator.depthTextureUsage() };
        // SAFETY: same instance and immutable property.
        let motion_usage = unsafe { interpolator.motionTextureUsage() };
        // SAFETY: same instance and immutable property.
        let output_usage = unsafe { interpolator.outputTextureUsage() };
        let colors = [
            texture(
                device,
                size,
                MTLPixelFormat::BGRA8Unorm,
                color_usage,
                "color-a",
            )?,
            texture(
                device,
                size,
                MTLPixelFormat::BGRA8Unorm,
                color_usage,
                "color-b",
            )?,
        ];
        let output = texture(
            device,
            size,
            MTLPixelFormat::BGRA8Unorm,
            output_usage,
            "output",
        )?;
        let depth = texture(
            device,
            size,
            MTLPixelFormat::R32Float,
            depth_usage,
            "virtual-depth",
        )?;
        let motion = texture(
            device,
            size,
            MTLPixelFormat::RG16Float,
            motion_usage,
            "motion",
        )?;
        let grid = texture(
            device,
            (size.0 / 8, size.1 / 8),
            MTLPixelFormat::RG16Float,
            MTLTextureUsage::ShaderRead,
            "motion-grid",
        )?;
        let library = device
            .newLibraryWithSource_options_error(
                &NSString::from_str(include_str!("../interpolation.msl")),
                None,
            )
            .map_err(|error| {
                log::warn!(target: "mtld3d::interpolation", "shader compilation failed: {error}");
                "Interpolation shader compilation failed; ordinary presentation is active."
            })?;
        let vertex = library
            .newFunctionWithName(&NSString::from_str("interpolation_vs"))
            .ok_or("Interpolation vertex shader is missing.")?;
        let pipeline = |name: &str, format| {
            let fragment = library.newFunctionWithName(&NSString::from_str(name))?;
            present::build_pipeline(device, &vertex, &fragment, format, name)
        };
        let search = pipeline("interpolation_search", MTLPixelFormat::RG16Float)
            .ok_or("Motion search pipeline failed.")?;
        let expand = pipeline("interpolation_motion", MTLPixelFormat::RG16Float)
            .ok_or("Motion expansion pipeline failed.")?;
        let plane = pipeline("interpolation_depth", MTLPixelFormat::R32Float)
            .ok_or("Synthetic depth pipeline failed.")?;
        Ok(Self {
            interpolator,
            colors,
            output,
            depth,
            motion,
            grid,
            search,
            expand,
            plane,
            index: 0,
        })
    }

    /// Snapshot current color and encode one midpoint, priming history on reset.
    pub fn encode(
        &mut self,
        cb: &ProtocolObject<dyn MTLCommandBuffer>,
        source: &ProtocolObject<dyn MTLTexture>,
        dt: f32,
        reset: bool,
    ) -> Option<Images> {
        let current = &self.colors[self.index];
        let previous = &self.colors[1 - self.index];
        if !command::encode_present_copy(cb, source, current) {
            return None;
        }
        if reset && !command::encode_present_copy(cb, source, previous) {
            return None;
        }
        pass(cb, &self.search, &self.grid, &[current, previous])?;
        pass(cb, &self.expand, &self.motion, &[&self.grid])?;
        pass(cb, &self.plane, &self.depth, &[])?;
        let fx = &self.interpolator;
        // SAFETY: all private textures have the exact descriptor geometry and required usage.
        unsafe { fx.setColorTexture(Some(current)) };
        // SAFETY: this is an independent snapshot of the previous encoded source image.
        unsafe { fx.setPrevColorTexture(Some(previous)) };
        // SAFETY: this explicit experimental plane matches the depth descriptor.
        unsafe { fx.setDepthTexture(Some(&self.depth)) };
        // SAFETY: the shader supplies current-to-previous motion in color-image pixels.
        unsafe { fx.setMotionTexture(Some(&self.motion)) };
        // SAFETY: output uses private storage and the usage bits MetalFX requested.
        unsafe { fx.setOutputTexture(Some(&self.output)) };
        // SAFETY: pixel displacements require unit scale.
        unsafe { fx.setMotionVectorScaleX(1.0) };
        // SAFETY: same coordinate convention for both axes.
        unsafe { fx.setMotionVectorScaleY(1.0) };
        // SAFETY: caller clamps dt to a finite, positive frame interval.
        unsafe { fx.setDeltaTime(dt) };
        // SAFETY: explicit virtual-camera parameters for this approximate-depth experiment.
        unsafe { fx.setNearPlane(0.1) };
        // SAFETY: far is greater than the positive near distance.
        unsafe { fx.setFarPlane(1000.0) };
        // SAFETY: valid vertical FOV in degrees; this is not a recovered FFXI camera.
        unsafe { fx.setFieldOfView(60.0) };
        let aspect = f32::from(u16::try_from(current.width()).ok()?)
            / f32::from(u16::try_from(current.height()).ok()?);
        // SAFETY: nonzero dimensions define the virtual image aspect.
        unsafe { fx.setAspectRatio(aspect) };
        // SAFETY: input is unjittered.
        unsafe { fx.setJitterOffsetX(0.0) };
        // SAFETY: input is unjittered.
        unsafe { fx.setJitterOffsetY(0.0) };
        // SAFETY: synthetic depth uses conventional Z explicitly, overriding Apple's default.
        unsafe { fx.setDepthReversed(false) };
        // SAFETY: no separate UI layer is supplied; UI is explicitly part of scene color.
        unsafe { fx.setIsUITextureComposited(false) };
        // SAFETY: previous holds last encoded source, or reset initializes both to current.
        unsafe { fx.setShouldResetHistory(reset) };
        // Keep the interpolator alive even if a settings change drops Resources before retirement.
        let retained = Mutex::new(Some(InterpolatorLease(
            Retained::into_raw(fx.clone()) as usize
        )));
        let release = RcBlock::new(move |_cb: NonNull<ProtocolObject<dyn MTLCommandBuffer>>| {
            // Taking the lease is idempotent. Dropping an uncommitted buffer also
            // drops this captured owner, so an abandoned encode cannot leak it.
            drop(retained.lock().unwrap().take());
        });
        // SAFETY: command buffer copies the handler; its lifetime includes GPU completion.
        unsafe { cb.addCompletedHandler(RcBlock::as_ptr(&release)) };
        // SAFETY: no encoder is open; all textures use tracked hazards on this one queue.
        unsafe { fx.encodeToCommandBuffer(cb) };
        self.index = 1 - self.index;
        Some(Images {
            real: current.clone(),
            generated: self.output.clone(),
        })
    }
}

fn texture(
    device: &ProtocolObject<dyn MTLDevice>,
    size: (usize, usize),
    format: MTLPixelFormat,
    usage: MTLTextureUsage,
    name: &str,
) -> Result<Retained<ProtocolObject<dyn MTLTexture>>, &'static str> {
    let descriptor = MTLTextureDescriptor::new();
    // SAFETY: caller supplies a nonzero width bounded to 1280 pixels.
    unsafe { descriptor.setWidth(size.0) };
    // SAFETY: caller supplies a nonzero height bounded to 720 pixels.
    unsafe { descriptor.setHeight(size.1) };
    descriptor.setPixelFormat(format);
    descriptor.setStorageMode(MTLStorageMode::Private);
    descriptor.setUsage(usage | MTLTextureUsage::ShaderRead | MTLTextureUsage::RenderTarget);
    let texture = device
        .newTextureWithDescriptor(&descriptor)
        .ok_or("Interpolation texture allocation failed.")?;
    texture.setLabel(Some(&NSString::from_str(&format!(
        "mtld3d-interpolation-{name}"
    ))));
    Ok(texture)
}

fn pass(
    cb: &ProtocolObject<dyn MTLCommandBuffer>,
    pipeline: &ProtocolObject<dyn MTLRenderPipelineState>,
    target: &ProtocolObject<dyn MTLTexture>,
    inputs: &[&ProtocolObject<dyn MTLTexture>],
) -> Option<()> {
    let descriptor = MTLRenderPassDescriptor::new();
    // SAFETY: Metal guarantees attachment slot zero exists.
    let attachment = unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
    attachment.setTexture(Some(target));
    attachment.setLoadAction(MTLLoadAction::DontCare);
    attachment.setStoreAction(MTLStoreAction::Store);
    let encoder = cb.renderCommandEncoderWithDescriptor(&descriptor)?;
    encoder.setLabel(Some(&NSString::from_str("mtld3d-interpolation-input")));
    encoder.setRenderPipelineState(pipeline);
    for (index, texture) in inputs.iter().enumerate() {
        // SAFETY: inputs are live retained textures; the command buffer retains GPU references.
        unsafe { encoder.setFragmentTexture_atIndex(Some(texture), index) };
    }
    // SAFETY: the vertex function generates its three positions without vertex buffers.
    unsafe { encoder.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::Triangle, 0, 3) };
    encoder.endEncoding();
    Some(())
}

/// A retained Metal object released on completion, or when an unused block drops.
struct InterpolatorLease(usize);

impl Drop for InterpolatorLease {
    fn drop(&mut self) {
        // SAFETY: the constructor transfers exactly one retain into this unique
        // owner. Metal object releases have no AppKit thread affinity.
        drop(unsafe {
            Retained::from_raw(self.0 as *mut ProtocolObject<dyn MTLFXFrameInterpolator>)
        });
    }
}
