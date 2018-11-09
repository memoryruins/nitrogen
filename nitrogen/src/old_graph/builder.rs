use std::collections::{HashMap, HashSet};

use graph;

use util::CowString;

#[derive(Default)]
pub struct GraphBuilder {
    pub(crate) enabled: bool,

    pub(crate) images_create: HashMap<CowString, graph::ImageCreateInfo>,
    pub(crate) images_copy: HashMap<CowString, CowString>,
    pub(crate) images_move: HashMap<CowString, CowString>,
    pub(crate) images_read: HashMap<CowString, u8>,
    pub(crate) images_write: HashMap<CowString, u8>,

    pub(crate) backbuffer_images: HashSet<CowString>,
}

impl GraphBuilder {
    pub(crate) fn new() -> Self {
        GraphBuilder::default()
    }

    pub fn enable(&mut self) {
        self.enabled = true;
    }

    pub fn image_create(&mut self, name: CowString, create_info: graph::ImageCreateInfo) {
        self.images_create.insert(name, create_info);
    }

    pub fn image_copy(&mut self, src: CowString, new: CowString) {
        self.images_copy.insert(new, src);
    }

    pub fn image_move(&mut self, src: CowString, new: CowString) {
        self.images_move.insert(src, new);
    }

    pub fn image_read(&mut self, name: CowString, binding: u8) {
        self.images_read.insert(name, binding);
    }

    pub fn image_write(&mut self, name: CowString, binding: u8) {
        self.images_write.insert(name, binding);
    }

    pub fn backbuffer_image(&mut self, name: CowString) {
        self.backbuffer_images.insert(name);
    }
}
