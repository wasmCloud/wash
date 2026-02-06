# k3s setup

Start everything:

```
docker compose up
```

Point kubectl to the generated kubeconfig:

```
export KUBECONFIG=$PWD/tmp/kubeconfig.yaml
```

You can now operate hosts/workloads with `kubectl`:

```
 ❯ kubectl get host
NAME                   HOSTID                                 HOSTGROUP   READY   AGE
forgetful-wound-2971   cf1ee307-cf74-4f69-b92f-a9eb593e478b   default     True    3m16s
```