{{/*
Common labels
*/}}
{{- define "matehub.labels" -}}
app.kubernetes.io/part-of: matehub
app.kubernetes.io/managed-by: {{ .Release.Service }}
helm.sh/chart: {{ .Chart.Name }}-{{ .Chart.Version }}
{{- end }}

{{/*
Image with registry and tag
*/}}
{{- define "matehub.image" -}}
{{ .global.registry }}/{{ .image.repository }}:{{ .image.tag | default .global.tag }}
{{- end }}
