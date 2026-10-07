-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#INTERMITTENT_GEN_SCADA#FILE01#202412.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS INTERMITTENT_GEN_SCADA owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#INTERMITTENT_GEN_SCADA#FILE01#202412010000.CSV
APPEND INTO TABLE INTERMITTENT_GEN_SCADA
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
RUN_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
DUID CHAR(20),
SCADA_TYPE CHAR(20),
SCADA_VALUE FLOAT EXTERNAL,
SCADA_QUALITY CHAR(20),
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS")
-- End of Script
