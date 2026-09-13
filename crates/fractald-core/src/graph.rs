use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::{ServiceSpec, ServiceType};

#[derive(Clone, Debug, Default)]
pub struct DependencyGraph {
    services: BTreeMap<String, ServiceSpec>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GraphError {
    DuplicateService(String),
    EmptyRoot,
    MissingRoot(String),
    MissingRequired { service: String, dependency: String },
    Cycle(Vec<String>),
}

impl DependencyGraph {
    pub fn add(&mut self, spec: ServiceSpec) -> Result<(), GraphError> {
        if self.services.contains_key(&spec.name) {
            return Err(GraphError::DuplicateService(spec.name));
        }
        self.services.insert(spec.name.clone(), spec);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&ServiceSpec> {
        self.services.get(name)
    }

    pub fn len(&self) -> usize {
        self.services.len()
    }

    pub fn is_empty(&self) -> bool {
        self.services.is_empty()
    }

    pub fn plan_start(&self, root: &str) -> Result<Vec<String>, GraphError> {
        let selected = self.select_start_closure(root)?;
        self.topological_order(&selected)
    }

    pub fn required_closure(&self, root: &str) -> Result<BTreeSet<String>, GraphError> {
        if root.is_empty() {
            return Err(GraphError::EmptyRoot);
        }
        if !self.services.contains_key(root) {
            return Err(GraphError::MissingRoot(root.to_owned()));
        }

        let mut required = BTreeSet::new();
        let mut pending = VecDeque::from([root.to_owned()]);
        while let Some(name) = pending.pop_front() {
            if !required.insert(name.clone()) {
                continue;
            }
            for dependency in self.required_dependencies(&name) {
                if !self.services.contains_key(&dependency) {
                    return Err(GraphError::MissingRequired {
                        service: name.clone(),
                        dependency: dependency.clone(),
                    });
                }
                pending.push_back(dependency.clone());
            }
        }
        Ok(required)
    }

    pub fn plan_stop(&self, root: &str) -> Result<Vec<String>, GraphError> {
        let mut order = self.plan_start(root)?;
        order.reverse();
        Ok(order)
    }

    pub fn plan_all_start(&self) -> Result<Vec<String>, GraphError> {
        let selected = self.services.keys().cloned().collect();
        self.topological_order(&selected)
    }

    pub fn plan_all_stop(&self) -> Result<Vec<String>, GraphError> {
        let mut order = self.plan_all_start()?;
        order.reverse();
        Ok(order)
    }

    pub fn plan_all_stop_best_effort(&self) -> Vec<String> {
        let selected = self.services.keys().cloned().collect::<BTreeSet<_>>();
        let (mut edges, mut indegree) = self.ordering_graph(&selected);
        let mut ready = indegree
            .iter()
            .filter_map(|(name, degree)| (degree == &0).then_some(name.clone()))
            .collect::<BTreeSet<_>>();
        let mut order = Vec::with_capacity(selected.len());

        while order.len() < selected.len() {
            let name = if let Some(name) = ready.pop_first() {
                name
            } else {
                let name = indegree
                    .iter()
                    .filter_map(|(name, degree)| (degree > &0).then_some(name.clone()))
                    .next_back()
                    .expect("a non-empty graph has a cycle endpoint");
                let predecessors = edges
                    .iter()
                    .filter_map(|(predecessor, dependents)| {
                        dependents.contains(&name).then_some(predecessor.clone())
                    })
                    .collect::<Vec<_>>();
                for predecessor in predecessors {
                    if edges
                        .get_mut(&predecessor)
                        .expect("cycle predecessor exists in edge map")
                        .remove(&name)
                    {
                        let degree = indegree
                            .get_mut(&name)
                            .expect("cycle endpoint exists in indegree map");
                        *degree -= 1;
                    }
                }
                ready.insert(name.clone());
                ready
                    .pop_first()
                    .expect("cycle endpoint was inserted into ready set")
            };

            order.push(name.clone());
            let dependents = std::mem::take(
                edges
                    .get_mut(&name)
                    .expect("order endpoint exists in edge map"),
            );
            for dependent in dependents {
                let degree = indegree
                    .get_mut(&dependent)
                    .expect("edge endpoint exists in indegree map");
                *degree -= 1;
                if *degree == 0 {
                    ready.insert(dependent);
                }
            }
        }

        order.reverse();
        order
    }

    fn select_start_closure(&self, root: &str) -> Result<BTreeSet<String>, GraphError> {
        if root.is_empty() {
            return Err(GraphError::EmptyRoot);
        }
        if !self.services.contains_key(root) {
            return Err(GraphError::MissingRoot(root.to_owned()));
        }

        let mut selected = BTreeSet::new();
        let mut pending = VecDeque::from([root.to_owned()]);
        while let Some(name) = pending.pop_front() {
            if !selected.insert(name.clone()) {
                continue;
            }
            let spec = &self.services[&name];
            for dependency in self.required_dependencies(&name) {
                if !self.services.contains_key(&dependency) {
                    return Err(GraphError::MissingRequired {
                        service: name.clone(),
                        dependency: dependency.clone(),
                    });
                }
                pending.push_back(dependency.clone());
            }
            for dependency in &spec.dependencies.wants {
                if self.services.contains_key(dependency) {
                    pending.push_back(dependency.clone());
                }
            }
            for dependency in self.wanted_mount_dependencies(&name) {
                pending.push_back(dependency);
            }
        }
        Ok(selected)
    }

    fn topological_order(&self, selected: &BTreeSet<String>) -> Result<Vec<String>, GraphError> {
        let (edges, mut indegree) = self.ordering_graph(selected);

        let mut ready: BTreeSet<String> = indegree
            .iter()
            .filter_map(|(name, degree)| (degree == &0).then_some(name.clone()))
            .collect();
        let mut order = Vec::with_capacity(selected.len());
        while let Some(name) = ready.pop_first() {
            order.push(name.clone());
            for dependent in &edges[&name] {
                let degree = indegree
                    .get_mut(dependent)
                    .expect("edge endpoint exists in indegree map");
                *degree -= 1;
                if *degree == 0 {
                    ready.insert(dependent.clone());
                }
            }
        }

        if order.len() != selected.len() {
            let cycle = selected
                .iter()
                .filter(|name| indegree[*name] != 0)
                .cloned()
                .collect();
            return Err(GraphError::Cycle(cycle));
        }
        Ok(order)
    }

    fn ordering_graph(
        &self,
        selected: &BTreeSet<String>,
    ) -> (BTreeMap<String, BTreeSet<String>>, BTreeMap<String, usize>) {
        let mut edges: BTreeMap<String, BTreeSet<String>> = selected
            .iter()
            .map(|name| (name.clone(), BTreeSet::new()))
            .collect();
        let mut indegree: BTreeMap<String, usize> =
            selected.iter().map(|name| (name.clone(), 0)).collect();

        for name in selected {
            let spec = &self.services[name];
            let required = self.required_dependencies(name);
            let wanted = self.wanted_dependencies(name);
            for dependency in required.iter().chain(wanted.iter()) {
                if selected.contains(dependency) {
                    add_edge(&mut edges, &mut indegree, dependency, name);
                }
            }
            for dependency in self.default_after_dependencies(name) {
                if selected.contains(&dependency) {
                    add_edge(&mut edges, &mut indegree, &dependency, name);
                }
            }
            for dependency in self.default_before_dependencies(name) {
                if selected.contains(&dependency) {
                    add_edge(&mut edges, &mut indegree, name, &dependency);
                }
            }
            for dependency in &spec.dependencies.after {
                if selected.contains(dependency) {
                    add_edge(&mut edges, &mut indegree, dependency, name);
                }
            }
            for dependency in &spec.dependencies.before {
                if selected.contains(dependency) {
                    add_edge(&mut edges, &mut indegree, name, dependency);
                }
            }
            for dependency in &spec.dependencies.requisite {
                if selected.contains(dependency) {
                    add_edge(&mut edges, &mut indegree, dependency, name);
                }
            }
        }

        (edges, indegree)
    }

    /// Return the explicit and path-derived required dependencies for a service.
    ///
    /// `RequiresMountsFor=` is represented as a path in a `ServiceSpec` because
    /// the corresponding mount unit may be discovered independently of the
    /// unit that refers to it.  Resolve those paths only after the complete
    /// registry is available, which also handles nested mount points.
    pub fn required_dependencies(&self, name: &str) -> BTreeSet<String> {
        let mut dependencies = self
            .services
            .get(name)
            .map(|service| service.dependencies.requires.clone())
            .unwrap_or_default();
        dependencies.extend(self.default_required_dependencies(name));
        if let Some(service) = self.services.get(name) {
            // BindsTo= has Requires= semantics in addition to propagating
            // later loss of the bound unit.
            dependencies.extend(service.dependencies.binds_to.iter().cloned());
        }
        dependencies.extend(self.mount_dependencies(name));
        dependencies
    }

    /// Return explicit and synthesized negative dependencies for a unit.
    pub fn conflict_dependencies(&self, name: &str) -> BTreeSet<String> {
        let mut dependencies = self
            .services
            .get(name)
            .map(|service| service.dependencies.conflicts.clone())
            .unwrap_or_default();
        dependencies.extend(self.default_conflict_dependencies(name));
        dependencies
    }

    fn default_required_dependencies(&self, name: &str) -> BTreeSet<String> {
        let _ = name;
        BTreeSet::new()
    }

    fn default_after_dependencies(&self, name: &str) -> BTreeSet<String> {
        let _ = name;
        BTreeSet::new()
    }

    fn default_before_dependencies(&self, name: &str) -> BTreeSet<String> {
        let _ = name;
        BTreeSet::new()
    }

    fn default_conflict_dependencies(&self, name: &str) -> BTreeSet<String> {
        let _ = name;
        BTreeSet::new()
    }

    fn mount_dependencies(&self, name: &str) -> BTreeSet<String> {
        let Some(service) = self.services.get(name) else {
            return BTreeSet::new();
        };
        let mut requested_paths = service
            .expanded_requires_mounts_for()
            .iter()
            .filter_map(|path| normalize_absolute_path(path))
            .collect::<Vec<_>>();
        if service.service_type == ServiceType::Mount {
            if let Some(path) = service
                .mount_where
                .as_ref()
                .and_then(|path| normalize_absolute_path(path))
            {
                requested_paths.push(path);
            }
        }
        if requested_paths.is_empty() {
            return BTreeSet::new();
        }

        self.services
            .iter()
            .filter_map(|(mount_name, mount)| {
                if mount_name == name || mount.service_type != ServiceType::Mount {
                    return None;
                }
                let mount_point = mount
                    .mount_where
                    .as_ref()
                    .and_then(|path| normalize_absolute_path(path))?;
                requested_paths
                    .iter()
                    .any(|path| path.starts_with(&mount_point))
                    .then_some(mount_name.clone())
            })
            .collect()
    }

    fn wanted_dependencies(&self, name: &str) -> BTreeSet<String> {
        let mut dependencies = self
            .services
            .get(name)
            .map(|service| service.dependencies.wants.clone())
            .unwrap_or_default();
        dependencies.extend(self.wanted_mount_dependencies(name));
        dependencies
    }

    fn wanted_mount_dependencies(&self, name: &str) -> BTreeSet<String> {
        let Some(service) = self.services.get(name) else {
            return BTreeSet::new();
        };
        let requested_paths = service
            .expanded_wants_mounts_for()
            .iter()
            .filter_map(|path| normalize_absolute_path(path))
            .collect::<Vec<_>>();
        if requested_paths.is_empty() {
            return BTreeSet::new();
        }

        self.services
            .iter()
            .filter_map(|(mount_name, mount)| {
                if mount_name == name || mount.service_type != ServiceType::Mount {
                    return None;
                }
                let mount_point = mount
                    .mount_where
                    .as_ref()
                    .and_then(|path| normalize_absolute_path(path))?;
                requested_paths
                    .iter()
                    .any(|path| path.starts_with(&mount_point))
                    .then_some(mount_name.clone())
            })
            .collect()
    }
}

fn normalize_absolute_path(path: &std::path::Path) -> Option<std::path::PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let mut normalized = std::path::PathBuf::from("/");
    for component in path.components() {
        match component {
            std::path::Component::RootDir | std::path::Component::CurDir => {}
            std::path::Component::Normal(value) => normalized.push(value),
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::Prefix(_) => return None,
        }
    }
    Some(normalized)
}

fn add_edge(
    edges: &mut BTreeMap<String, BTreeSet<String>>,
    indegree: &mut BTreeMap<String, usize>,
    from: &str,
    to: &str,
) {
    if edges
        .get_mut(from)
        .expect("source endpoint exists in edge map")
        .insert(to.to_owned())
    {
        *indegree
            .get_mut(to)
            .expect("destination endpoint exists in indegree map") += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DependencySet;

    fn service(name: &str) -> ServiceSpec {
        ServiceSpec::new(name, format!("/usr/libexec/{name}"))
    }

    #[test]
    fn plans_required_and_wanted_services_before_root() {
        let mut graph = DependencyGraph::default();
        let mut root = service("app");
        root.dependencies.requires.insert("db".to_owned());
        root.dependencies.wants.insert("metrics".to_owned());
        graph.add(root).expect("root");
        graph.add(service("db")).expect("database");
        graph.add(service("metrics")).expect("metrics");

        let plan = graph.plan_start("app").expect("start plan");
        assert_eq!(plan, vec!["db", "metrics", "app"]);
        assert_eq!(
            graph.plan_stop("app").expect("stop plan"),
            vec!["app", "metrics", "db"]
        );
    }

    #[test]
    fn missing_wants_are_ignored_but_missing_requires_fail() {
        let mut graph = DependencyGraph::default();
        let mut root = service("app");
        root.dependencies.wants.insert("optional".to_owned());
        root.dependencies.requires.insert("database".to_owned());
        graph.add(root).expect("root");

        assert_eq!(
            graph.plan_start("app"),
            Err(GraphError::MissingRequired {
                service: "app".to_owned(),
                dependency: "database".to_owned(),
            })
        );

        let mut graph = DependencyGraph::default();
        let mut root = service("app");
        root.dependencies.wants.insert("optional".to_owned());
        graph.add(root).expect("root");
        assert_eq!(graph.plan_start("app").expect("start plan"), vec!["app"]);
    }

    #[test]
    fn requisite_does_not_pull_an_inactive_unit_into_the_start_plan() {
        let mut graph = DependencyGraph::default();
        let mut application = service("application");
        application
            .dependencies
            .requisite
            .insert("prerequisite".to_owned());
        graph.add(application).expect("application");
        graph.add(service("prerequisite")).expect("prerequisite");

        assert_eq!(
            graph.plan_start("application").expect("start plan"),
            vec!["application"]
        );
    }

    #[test]
    fn detects_ordering_cycles() {
        let mut graph = DependencyGraph::default();
        let mut first = service("first");
        first.dependencies.after.insert("second".to_owned());
        first.dependencies.wants.insert("second".to_owned());
        let mut second = service("second");
        second.dependencies.after.insert("first".to_owned());
        graph.add(first).expect("first");
        graph.add(second).expect("second");

        assert_eq!(
            graph.plan_start("first"),
            Err(GraphError::Cycle(vec![
                "first".to_owned(),
                "second".to_owned()
            ]))
        );
    }

    #[test]
    fn best_effort_shutdown_breaks_cycles_deterministically() {
        let mut graph = DependencyGraph::default();
        let mut first = service("first");
        first.dependencies.after.insert("second".to_owned());
        let mut second = service("second");
        second.dependencies.after.insert("first".to_owned());
        graph.add(first).expect("first");
        graph.add(second).expect("second");

        assert_eq!(
            graph.plan_all_stop_best_effort(),
            vec!["first".to_owned(), "second".to_owned()]
        );
    }

    #[test]
    fn before_is_the_reverse_of_after() {
        let mut graph = DependencyGraph::default();
        let mut first = service("first");
        first.dependencies.before.insert("second".to_owned());
        graph.add(first).expect("first");
        graph.add(service("second")).expect("second");

        assert_eq!(
            graph.plan_start("first").expect("start plan"),
            vec!["first"]
        );

        let mut second = service("second");
        second.dependencies.wants.insert("first".to_owned());
        second.dependencies.after.insert("first".to_owned());
        let mut graph = DependencyGraph::default();
        graph.add(service("first")).expect("first");
        graph.add(second).expect("second");
        assert_eq!(
            graph.plan_start("second").expect("start plan"),
            vec!["first", "second"]
        );
    }

    #[test]
    fn dependency_set_defaults_empty() {
        assert_eq!(service("demo").dependencies, DependencySet::default());
    }

    #[test]
    fn requires_mounts_for_adds_nested_mounts_to_the_required_closure() {
        let mut graph = DependencyGraph::default();
        let mut application = service("application");
        application
            .requires_mounts_for
            .push("/srv/data/cache/file".into());
        graph.add(application).expect("application");

        let mut data = service("srv-data.mount");
        data.service_type = ServiceType::Mount;
        data.mount_where = Some("/srv/data".into());
        graph.add(data).expect("data mount");
        let mut cache = service("srv-data-cache.mount");
        cache.service_type = ServiceType::Mount;
        cache.mount_where = Some("/srv/data/cache".into());
        graph.add(cache).expect("cache mount");

        assert_eq!(
            graph.plan_start("application").expect("start plan"),
            vec![
                "srv-data.mount".to_owned(),
                "srv-data-cache.mount".to_owned(),
                "application".to_owned(),
            ]
        );
    }

    #[test]
    fn wants_mounts_for_adds_nested_mounts_without_making_them_required() {
        let mut graph = DependencyGraph::default();
        let mut application = service("application");
        application
            .wants_mounts_for
            .push("/srv/data/cache/file".into());
        graph.add(application).expect("application");

        let mut data = service("srv-data.mount");
        data.service_type = ServiceType::Mount;
        data.mount_where = Some("/srv/data".into());
        graph.add(data).expect("data mount");
        let mut cache = service("srv-data-cache.mount");
        cache.service_type = ServiceType::Mount;
        cache.mount_where = Some("/srv/data/cache".into());
        graph.add(cache).expect("cache mount");

        assert_eq!(
            graph.plan_start("application").expect("start plan"),
            vec![
                "srv-data.mount".to_owned(),
                "srv-data-cache.mount".to_owned(),
                "application".to_owned(),
            ]
        );
        assert_eq!(
            graph
                .required_closure("application")
                .expect("required closure"),
            ["application".to_owned()].into()
        );
    }

    #[test]
    fn native_graph_does_not_inject_implicit_manager_services() {
        let mut graph = DependencyGraph::default();
        graph
            .add(service("shutdown.profile"))
            .expect("shutdown profile");
        graph.add(service("boot.profile")).expect("boot profile");
        graph.add(service("worker.svc")).expect("worker");

        assert_eq!(
            graph.plan_start("worker.svc").expect("start plan"),
            vec!["worker.svc".to_owned()]
        );
        assert!(graph.conflict_dependencies("worker.svc").is_empty());
    }

    #[test]
    fn default_dependencies_no_disables_synthesized_edges() {
        let mut graph = DependencyGraph::default();
        graph.add(service("boot.profile")).expect("boot profile");
        graph
            .add(service("shutdown.profile"))
            .expect("shutdown profile");
        let mut worker = service("early.svc");
        worker.default_dependencies = false;
        graph.add(worker).expect("early service");

        assert_eq!(
            graph.required_closure("early.svc").expect("closure"),
            ["early.svc".to_owned()].into()
        );
        assert!(
            !graph
                .conflict_dependencies("early.service")
                .contains("shutdown.target")
        );
    }
}
