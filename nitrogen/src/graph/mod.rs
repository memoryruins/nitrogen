/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

use crate::util::storage::{Handle, Storage};
use crate::util::CowString;

use std::collections::HashMap;

use smallvec::SmallVec;

use crate::device::DeviceContext;
use crate::resources::{
    buffer::BufferStorage,
    command_pool::{CommandPoolCompute, CommandPoolGraphics},
    image::ImageStorage,
    material::MaterialStorage,
    pipeline::PipelineStorage,
    render_pass::RenderPassStorage,
    sampler::SamplerStorage,
    semaphore_pool::{SemaphoreList, SemaphorePool},
    vertex_attrib::VertexAttribStorage,
};

pub mod pass;
pub use self::pass::*;

pub mod builder;
pub use self::builder::*;

pub mod command;
pub use self::command::*;

pub(crate) mod compilation;
pub(crate) use self::compilation::*;
pub(crate) mod execution;
pub(crate) use self::execution::*;

pub use self::execution::Backbuffer;

pub mod store;
pub use self::store::*;

use crate::submit_group::ResourceList;

pub(crate) struct Storages<'a> {
    pub render_pass: &'a mut RenderPassStorage,
    pub pipeline: &'a mut PipelineStorage,
    pub image: &'a mut ImageStorage,
    pub buffer: &'a mut BufferStorage,
    pub vertex_attrib: &'a VertexAttribStorage,
    pub sampler: &'a mut SamplerStorage,
    pub material: &'a mut MaterialStorage,
}

pub type GraphHandle = Handle<Graph>;

pub type PassName = CowString;
pub type ResourceName = CowString;

#[derive(Default)]
pub struct Graph {
    passes: Vec<(PassName, PassInfo)>,
    passes_gfx_impl: HashMap<usize, Box<dyn GraphicsPassImpl>>,
    passes_cmpt_impl: HashMap<usize, Box<dyn ComputePassImpl>>,
    pub(crate) output_resources: Vec<ResourceName>,

    pub(crate) resolve_cache: HashMap<u64, (GraphResourcesResolved, usize)>,
    exec_id_cache: HashMap<(usize, Vec<ResourceName>), usize>,
    exec_graph_cache: HashMap<usize, ExecutionGraph>,

    exec_usages: HashMap<usize, ResourceUsages>,
    pub(crate) exec_base_resources: HashMap<usize, GraphBaseResources>,

    last_exec: Option<usize>,
    pub(crate) last_input: Option<u64>,
}

pub(crate) struct GraphStorage {
    pub(crate) storage: Storage<Graph>,
}

impl GraphStorage {
    pub(crate) fn new() -> Self {
        GraphStorage {
            storage: Storage::new(),
        }
    }

    pub(crate) fn create(&mut self) -> GraphHandle {
        self.storage.insert(Graph::default())
    }

    pub(crate) fn destroy(
        &mut self,
        res_list: &mut ResourceList,
        storages: &mut Storages,
        handle: GraphHandle,
    ) {
        let graph = self.storage.remove(handle);

        if let Some(graph) = graph {
            for (_, res) in graph.exec_base_resources {
                res.release(res_list, storages);
            }
        }
    }

    pub(crate) fn add_graphics_pass<T: Into<PassName>>(
        &mut self,
        handle: GraphHandle,
        name: T,
        pass_info: GraphicsPassInfo,
        pass_impl: Box<dyn GraphicsPassImpl>,
    ) {
        self.storage.get_mut(handle).map(|graph| {
            let id = graph.passes.len();
            graph
                .passes
                .push((name.into(), PassInfo::Graphics(pass_info)));
            graph.passes_gfx_impl.insert(id, pass_impl);
        });
    }

    pub(crate) fn add_compute_pass<T: Into<PassName>>(
        &mut self,
        handle: GraphHandle,
        name: T,
        pass_info: ComputePassInfo,
        pass_impl: Box<dyn ComputePassImpl>,
    ) {
        self.storage.get_mut(handle).map(|graph| {
            let id = graph.passes.len();
            graph
                .passes
                .push((name.into(), PassInfo::Compute(pass_info)));
            graph.passes_cmpt_impl.insert(id, pass_impl);
        });
    }

    /// Compile the graph so it is optimized for execution.
    ///
    /// This runs all the `setup` methods on all passes (`GraphicsPassImpl`, `ComputePassImpl`)
    /// and checks if the "same graph version" has been encountered before. If so, the old compiled
    /// representation can be reused.
    ///
    /// If this is a new graph permutation then all the resource names are resolved to IDs and lists
    /// are created that store which pass creates, reads, writes or moves which resource.
    ///
    /// A "execution path" representation is then created from that information, only including
    /// relevant passes for the current permutation.
    ///
    /// With that in place, any gfx resources are created that won't change across execution
    /// (like pipelines and render passes)
    pub(crate) fn compile(
        &mut self,
        backbuffer_usage: &mut BackbufferUsage,
        store: &mut Store,
        handle: GraphHandle,
    ) -> Result<(), Vec<GraphCompileError>> {
        let graph = self
            .storage
            .get_mut(handle)
            .ok_or(vec![GraphCompileError::InvalidGraph])?;

        let mut input = GraphInput::default();

        for (i, pass) in graph.passes_gfx_impl.iter_mut() {
            let mut builder = GraphBuilder::new();
            pass.setup(store, &mut builder);

            let id = PassId(*i);

            if builder.enabled {
                input.add_builder(id, builder);
            }
        }

        for (i, pass) in graph.passes_cmpt_impl.iter_mut() {
            let mut builder = GraphBuilder::new();
            pass.setup(store, &mut builder);

            let id = PassId(*i);

            if builder.enabled {
                input.add_builder(id, builder);
            }
        }

        // TODO hash the above things and make a lookup table to spare doing the work below.

        let mut errors = vec![];
        let mut read_types = vec![];
        let mut write_types = vec![];

        // "reverse the arrows"

        let input_hash = {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};

            let mut hasher = DefaultHasher::new();

            input.hash(&mut hasher);

            hasher.finish()
        };

        let resolve_cache = &mut graph.resolve_cache;

        let resolved_id = resolve_cache.len();

        let (resolved, resolved_id) = resolve_cache.entry(input_hash).or_insert_with(|| {
            (
                resolve_input_graph(input, &mut read_types, &mut write_types, &mut errors),
                resolved_id,
            )
        });

        graph.last_input = Some(input_hash);

        // TODO check write and read types match with creation types.

        let exec_id = graph.exec_id_cache.len();

        let exec_id = *graph
            .exec_id_cache
            .entry((*resolved_id, graph.output_resources.clone()))
            .or_insert(exec_id);

        // if this backbuffer has not been used with this kind of graph yet,
        // set it up to work with it!
        if backbuffer_usage.exec_ids.contains(&exec_id) {
            backbuffer_usage.exec_ids.insert(exec_id);
            backbuffer_usage.add_graph(resolved);
        }

        let output_slice = graph.output_resources.as_slice();

        for output in output_slice {
            if !resolved.name_lookup.contains_key(output) {
                errors.push(GraphCompileError::InvalidOutputResource {
                    res: output.clone(),
                });
            }
        }

        graph
            .exec_graph_cache
            .entry(exec_id)
            .or_insert_with(|| ExecutionGraph::new(resolved, output_slice));

        graph.last_exec = Some(exec_id);

        if !errors.is_empty() {
            Err(errors)
        } else {
            Ok(())
        }
    }

    pub(crate) unsafe fn execute(
        &mut self,
        device: &DeviceContext,
        sem_pool: &mut SemaphorePool,
        sem_list: &mut SemaphoreList,
        cmd_pool_gfx: &CommandPoolGraphics,
        cmd_pool_cmpt: &CommandPoolCompute,
        res_list: &mut ResourceList,
        storages: &mut Storages,
        store: &Store,
        graph_handle: GraphHandle,
        backbuffer: &mut Backbuffer,
        prev_res: Option<GraphResources>,
        context: &ExecutionContext,
    ) -> Option<GraphResources> {
        // TODO error handling !!!
        // TODO
        // TODO change this ^ into Result<>
        let graph = self.storage.get_mut(graph_handle)?;

        let input_hash = graph.last_input?;

        let (resolved, id) = graph.resolve_cache.get(&input_hash)?;

        let exec_id = graph
            .exec_id_cache
            .get(&(*id, graph.output_resources.clone()))?;

        let mut remake_resources = false;

        // The backbuffer has not been used with this kind of graph yet,
        // make sure it works!
        if !backbuffer.usage.exec_ids.contains(exec_id) {
            backbuffer.usage.exec_ids.insert(*exec_id);
            backbuffer.usage.add_graph(resolved);
            remake_resources = true;
        }

        // At this point the backbuffer is compatible with the current graph
        // but it might not have been made working with the currently *existing*
        // graph configuration (exec + context).
        //
        // So we mark the graph as compatible and remove all resources
        // and remake them.
        if backbuffer.exec_contexts.get(exec_id) != Some(context) {
            backbuffer.exec_contexts.insert(*exec_id, context.clone());
            backbuffer.clear(storages, res_list);
            remake_resources = true;
        }

        let exec = &graph.exec_graph_cache[exec_id];

        let outputs = graph
            .output_resources
            .as_slice()
            .iter()
            .map(|n| resolved.name_lookup[n])
            .collect::<SmallVec<[_; 16]>>();

        let resources = {
            let (base_resources, usages) = {
                let passes = &graph.passes[..];

                // insert into cache
                graph
                    .exec_base_resources
                    .entry(*exec_id)
                    .or_insert_with(|| {
                        execution::prepare_base(
                            device,
                            &backbuffer.usage,
                            storages,
                            exec,
                            resolved,
                            passes,
                        )
                    });

                // now read base again (some kind of reborrowing, need to investigate...)
                let base = graph.exec_base_resources.get_mut(exec_id)?;

                graph.exec_usages.entry(*exec_id).or_insert_with(|| {
                    execution::derive_resource_usage(
                        &backbuffer.usage,
                        exec,
                        resolved,
                        outputs.as_slice(),
                    )
                });

                let usages = &graph.exec_usages[exec_id];

                (base, usages)
            };

            match prev_res {
                None => {
                    // create new completely!
                    let mut res = prepare(
                        usages,
                        backbuffer,
                        base_resources,
                        device,
                        storages,
                        exec,
                        resolved,
                        graph.passes.as_slice(),
                        context.clone(),
                    )?;

                    // add the resolved outputs
                    res.outputs = outputs;
                    res.exec_version = *exec_id;
                    res
                }
                Some(res) => {
                    if Some(res.exec_version) == graph.last_exec
                        && &res.exec_context == context
                        && !remake_resources
                    {
                        // same old resources, we can keep them!

                        res
                    } else {
                        // recreate resources
                        res.release(res_list, storages);

                        let mut res = prepare(
                            usages,
                            backbuffer,
                            base_resources,
                            device,
                            storages,
                            exec,
                            resolved,
                            graph.passes.as_slice(),
                            context.clone(),
                        )?;

                        // add the resolved outputs
                        res.outputs = outputs;
                        res.exec_version = *exec_id;
                        res
                    }
                }
            }
        };

        execution::execute(
            device,
            sem_pool,
            sem_list,
            cmd_pool_gfx,
            cmd_pool_cmpt,
            storages,
            store,
            exec,
            resolved,
            graph,
            &graph.exec_base_resources[exec_id],
            &resources,
            context,
        );

        Some(resources)
    }

    pub(crate) fn add_output<T: Into<ResourceName>>(&mut self, handle: GraphHandle, image: T) {
        self.storage.get_mut(handle).map(|graph| {
            graph.output_resources.push(image.into());
        });
    }

    /*
    pub(crate) fn output_buffer<T: Into<ResourceName>>(
        &self,
        handle: GraphHandle,
        buffer: T,
    ) -> Option<crate::buffer::BufferHandle> {
        let graph = self.storage.get(handle)?;
        let in_num = graph.last_input?;
        let (resolve, _exec_num) = graph.resolve_cache.get(&in_num)?;
        let id = *resolve.name_lookup.get(&buffer.into())?;
        let (_, res) = graph.exec_resources.as_ref()?;

        res.buffers.get(&id).map(|x| *x)
    }
    */
}

#[derive(Debug, Clone, Ord, PartialOrd, PartialEq, Eq)]
pub struct ExecutionContext {
    pub reference_size: (u32, u32),
}
