/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

mod derive;
mod execute;
mod graph;
mod prepare;

pub(crate) use self::derive::*;
pub(crate) use self::execute::*;
pub(crate) use self::graph::*;
pub(crate) use self::prepare::*;

use super::{PassId, PassName, ResourceId, Storages};
use crate::resources::{
    buffer::BufferHandle, image::ImageHandle, pipeline::PipelineHandle,
    render_pass::RenderPassHandle, sampler::SamplerHandle,
};
use crate::types;

use crate::submit_group::ResourceList;

use std::collections::HashMap;

use smallvec::SmallVec;

use gfx;

#[derive(Debug, Default)]
pub(crate) struct ResourceUsages {
    image: HashMap<ResourceId, (gfx::image::Usage, gfx::format::Format)>,
    buffer: HashMap<ResourceId, gfx::buffer::Usage>,
}

#[derive(Debug, Default)]
pub(crate) struct GraphBaseResources {
    render_passes: HashMap<PassId, RenderPassHandle>,

    pipelines_graphic: HashMap<PassId, PipelineHandle>,
    pipelines_compute: HashMap<PassId, PipelineHandle>,
    pub(crate) pipelines_mat: HashMap<PassId, crate::material::MaterialHandle>,
}

impl GraphBaseResources {
    pub(crate) fn release(self, res_list: &mut ResourceList, storages: &mut Storages) {
        storages
            .render_pass
            .destroy(res_list, self.render_passes.values());

        storages
            .pipeline
            .destroy(res_list, self.pipelines_graphic.values());

        storages
            .pipeline
            .destroy(res_list, self.pipelines_compute.values());

        // TODO free pass materials
    }
}

#[derive(Debug)]
pub(crate) struct GraphResources {
    pub(crate) exec_version: usize,
    pub(crate) exec_context: super::ExecutionContext,

    pub(crate) images: HashMap<ResourceId, ImageHandle>,
    samplers: HashMap<ResourceId, SamplerHandle>,
    pub(crate) buffers: HashMap<ResourceId, BufferHandle>,

    framebuffers: HashMap<PassId, (types::Framebuffer, gfx::image::Extent)>,

    pass_mats: HashMap<PassId, crate::material::MaterialInstanceHandle>,

    pub(crate) outputs: SmallVec<[ResourceId; 16]>,
}

impl GraphResources {
    pub(crate) fn release(self, res_list: &mut ResourceList, storages: &mut Storages) {
        storages.image.destroy(res_list, self.images.values());

        storages.sampler.destroy(res_list, self.samplers.values());

        storages.buffer.destroy(res_list, self.buffers.values());

        for (_, (fb, _)) in self.framebuffers {
            res_list.queue_framebuffer(fb);
        }

        for mat_instance in self.pass_mats.values() {
            res_list.queue_material_instance(*mat_instance);
        }
    }
}
