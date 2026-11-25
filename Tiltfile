# Tiltfile: run the clax fleet on a local Kubernetes cluster and watch it.
#
#   tilt up                                   the example overlay (deploy/tilt/example)
#   CLAX_TILT_OVERLAY=~/my-clax tilt up       an overlay of your own (docs/kubernetes.md, Deploying)
#
# Deploys the published clax-fleet image the overlay names (nothing is built
# here), reapplies when the catalog or the manifests change (the pods reload the
# catalog in place, no restart), and shows the StatefulSet's pods, logs and
# readiness in Tilt's UI (http://localhost:10350).
#
# Not Tilt's: the clax-secrets Secret (create it once, as docs/kubernetes.md
# says; Tilt never sees its values) and anything cluster-wide your ingress needs.
#
# Tilt refuses a cluster it does not know is local. kind, k3d, minikube and
# Docker Desktop it knows; for any other (Lima's k3s calls its context
# `default`) name it: CLAX_TILT_CONTEXTS=default.

overlay = os.getenv('CLAX_TILT_OVERLAY', 'deploy/tilt/example')
contexts = os.getenv('CLAX_TILT_CONTEXTS', '')
if contexts:
    allow_k8s_contexts(contexts.split(','))

k8s_yaml(kustomize(overlay))

# Each instance's own page, for the UI's links: CLAX_TILT_LINKS=https://a.example,https://b.example
links = [l for l in os.getenv('CLAX_TILT_LINKS', '').split(',') if l]
k8s_resource('clax', labels=['clax'], links=links)
