-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#FPP_PD_FWD_EST_RESIDUALRATE#FILE01#202501.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS FPP_PD_FWD_EST_RESIDUALRATE owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#FPP_PD_FWD_EST_RESIDUALRATE#FILE01#202501010000.CSV
APPEND INTO TABLE FPP_PD_FWD_EST_RESIDUALRATE
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
PREDISPATCHSEQNO CHAR(20),
RUN_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
RUNNO FLOAT EXTERNAL,
INTERVAL_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
CONSTRAINTID CHAR(20),
VERSIONNO FLOAT EXTERNAL,
BIDTYPE CHAR(10),
RELEVANT_REGIONS CHAR(200),
EST_UNUSED_FCAS FLOAT EXTERNAL)
-- End of Script
