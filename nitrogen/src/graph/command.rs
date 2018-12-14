/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

use std::ops::Range;

use gfx;
use gfx::command;

use crate::types;

use crate::material::{MaterialInstanceHandle, MaterialStorage};

use crate::buffer::{BufferHandle, BufferStorage};

#[derive(Clone)]
pub(crate) struct ReadStorages<'a> {
    pub(crate) buffer: &'a BufferStorage,
    pub(crate) material: &'a MaterialStorage,
}

pub struct GraphicsCommandBuffer<'a> {
    pub(crate) encoder:
        gfx::command::RenderPassInlineEncoder<'a, back::Backend, gfx::command::Primary>,
    pub(crate) storages: &'a ReadStorages<'a>,

    pub(crate) pipeline_layout: &'a types::PipelineLayout,
}

impl<'a> GraphicsCommandBuffer<'a> {
    pub fn draw(&mut self, vertices: Range<u32>, instances: Range<u32>) {
        self.encoder.draw(vertices, instances);
    }

    /// Bind vertex buffers for the next draw call.
    /// The provided pairs of buffer and `usize` represent the buffer to bind
    /// and the **offset into the buffer**.
    /// The first pair will be bound to vertex buffer 0, the second to 1, etc...
    pub fn bind_vertex_buffers<T, I>(&mut self, buffers: T)
    where
        T: IntoIterator<Item = I>,
        T::Item: std::borrow::Borrow<(BufferHandle, usize)>,
    {
        let stores = self.storages.clone();

        let bufs = buffers.into_iter().filter_map(|i| {
            let (buffer, index) = i.borrow();
            stores
                .buffer
                .raw(*buffer)
                .map(|buf| (buf.raw(), *index as u64))
        });

        self.encoder.bind_vertex_buffers(0, bufs);
    }

    pub fn bind_material(
        &mut self,
        binding: usize,
        material: MaterialInstanceHandle,
    ) -> Option<()> {
        let layout = self.pipeline_layout;

        let mat = self.storages.material.raw(material.0)?;
        let instance = mat.intance_raw(material.1)?;

        let set = &instance.set;

        self.encoder
            .bind_graphics_descriptor_sets(layout, binding, Some(set), &[]);

        Some(())
    }
}

pub struct ComputeCommandBuffer<'a> {
    pub(crate) buf:
        command::CommandBuffer<'a, back::Backend, gfx::Compute, command::OneShot, command::Primary>,
    pub(crate) storages: &'a ReadStorages<'a>,

    pub(crate) pipeline_layout: &'a types::PipelineLayout,
}

impl<'a> ComputeCommandBuffer<'a> {
    pub fn dispatch(&mut self, workgroup_count: [u32; 3]) {
        self.buf.dispatch(workgroup_count)
    }

    pub fn bind_material(
        &mut self,
        binding: usize,
        material: MaterialInstanceHandle,
    ) -> Option<()> {
        let layout = self.pipeline_layout;

        let mat = self.storages.material.raw(material.0)?;
        let instance = mat.intance_raw(material.1)?;

        let set = &instance.set;

        self.buf
            .bind_compute_descriptor_sets(layout, binding, Some(set), &[]);

        Some(())
    }
}
