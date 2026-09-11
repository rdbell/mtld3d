//! Conservative scheduling of independent single-sample render passes.

use super::{
    ColorLoad, Command, CommandType, DepthLoad, FxHashMap, FxHashSet, MTLTextureKind, MetalHandle,
    Pass, PassState, StencilLoad,
};

struct Access {
    reads: FxHashSet<u64>,
    writes: FxHashSet<u64>,
    movable: bool,
}

impl Access {
    fn new(
        pass: &Pass,
        twins: &FxHashMap<MetalHandle<MTLTextureKind>, MetalHandle<MTLTextureKind>>,
    ) -> Self {
        let mut result = Self {
            reads: FxHashSet::default(),
            writes: [pass.color_texture.raw(), pass.depth_texture.raw()]
                .into_iter()
                .filter(|h| *h != 0)
                .collect(),
            movable: pass.leading_blits.is_empty()
                && !pass.has_counting_visibility
                && pass.extra_color.iter().all(|a| !a.is_bound())
                && pass.color_msaa_texture.is_null()
                && pass.color_resolve_texture.is_null()
                && pass.depth_resolve_texture.is_null(),
        };
        for command in &pass.commands {
            match CommandType::from_repr(command.cmd) {
                Some(CommandType::SetFragmentTexture | CommandType::SetVertexTexture) => {
                    if command.param_b != 0 {
                        // SAFETY: both texture-bind commands carry an MTLTexture handle in param_b.
                        let handle = unsafe { MetalHandle::new(command.param_b) };
                        result
                            .reads
                            .insert(twins.get(&handle).unwrap_or(&handle).raw());
                    }
                }
                // D3D9 shaders only read buffers. Uploads are either already ordered
                // before all passes or in leading_blits, which is a barrier above.
                Some(
                    CommandType::SetRenderPipelineState
                    | CommandType::SetViewport
                    | CommandType::SetVertexBytes
                    | CommandType::DrawPrimitives
                    | CommandType::SetDepthStencilState
                    | CommandType::SetCullMode
                    | CommandType::SetFragmentSamplerState
                    | CommandType::SetVertexBytesAt
                    | CommandType::SetFragmentBytesAt
                    | CommandType::SetScissorRect
                    | CommandType::SetVertexBuffer
                    | CommandType::DrawIndexedPrimitives
                    | CommandType::SetBlendColor
                    | CommandType::SetDepthBias
                    | CommandType::DrawIndexedPrimitivesUp
                    | CommandType::SetStencilReference
                    | CommandType::SetFragmentNullTexture
                    | CommandType::SetVertexSamplerState
                    | CommandType::SetVertexNullTexture
                    | CommandType::SetFragmentBuffer
                    | CommandType::PushDebugGroup
                    | CommandType::PopDebugGroup,
                ) => {}
                Some(CommandType::SetVisibilityResultMode) | None => result.movable = false,
            }
        }
        // Sampling an attachment can require a completed earlier render pass.
        result.movable &= result.reads.is_disjoint(&result.writes);
        result
    }

    fn independent_of(&self, other: &Self) -> bool {
        self.movable
            && other.movable
            && self.writes.is_disjoint(&other.writes)
            && self.reads.is_disjoint(&other.writes)
            && self.writes.is_disjoint(&other.reads)
    }
}

fn compatible(first: &Pass, next: &Pass) -> bool {
    first.color_texture == next.color_texture
        && first.color_srgb_texture == next.color_srgb_texture
        && first.color_subresource == next.color_subresource
        && first.color_size == next.color_size
        && first.color_format == next.color_format
        && first.depth_texture == next.depth_texture
        && first.depth_level == next.depth_level
        && first.depth_is_sampleable == next.depth_is_sampleable
        && first.viewport == next.viewport
        && (next.color_texture.is_null() || next.color_load == ColorLoad::Load)
        && (next.depth_texture.is_null()
            || (next.depth_load == DepthLoad::Load && next.stencil_load == StencilLoad::Load))
}

impl PassState {
    /// Merge later draws into an earlier pass only across independent texture accesses.
    ///
    /// Each original pass keeps its draw order. Clears, copies, resolves, visibility
    /// commands, attachment feedback, and unknown commands prevent unsafe movement.
    /// Run before load/store finalization, which must see the resulting pass order.
    pub fn merge_independent_draw_passes(&mut self) {
        let before = self.passes.len();
        let mut accesses: Vec<_> = self
            .passes
            .iter()
            .map(|p| Access::new(p, &self.srgb_twin_to_base))
            .collect();
        let mut index = 1;
        while index < self.passes.len() {
            let mut target = None;
            if accesses[index].movable {
                // A fixed lookback bounds scheduling overhead on pathological frames.
                for earlier in (index.saturating_sub(16)..index).rev() {
                    if accesses[earlier].movable
                        && compatible(&self.passes[earlier], &self.passes[index])
                    {
                        target = Some(earlier);
                        break;
                    }
                    if !accesses[index].independent_of(&accesses[earlier]) {
                        break;
                    }
                }
            }
            let Some(target) = target else {
                index += 1;
                continue;
            };
            let mut next = self.passes.remove(index);
            let next_access = accesses.remove(index);
            let first = &mut self.passes[target];
            // LastBoundCache suppresses these fresh-encoder defaults. Recreate
            // the old pass boundary's state before replaying its commands.
            first.commands.extend([
                Command::set_blend_color(1.0, 1.0, 1.0, 1.0),
                Command::set_depth_bias(0.0, 0.0),
                Command::set_stencil_reference(0),
            ]);
            let offset = first.commands.len();
            first.commands.append(&mut next.commands);
            first.color_clear_quad_ranges.extend(
                next.color_clear_quad_ranges
                    .into_iter()
                    .map(|(start, end)| (start + offset, end + offset)),
            );
            first.color_writes_observed |= next.color_writes_observed;
            accesses[target].reads.extend(next_access.reads);
        }
        log::trace!(target: "mtld3d::pass_merge", "passes_before={before} passes_after={}", self.passes.len());
    }
}
